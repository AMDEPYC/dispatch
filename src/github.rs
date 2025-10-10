use std::collections::BTreeSet;
use std::process::Command;

use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Dispatch content types
///
/// The purpose of this type is to map dispatch content types to UEFI content
/// types. This means that GitHub can only select a subset of assets as
/// dispatch targets. Dispatch will then automatically handle the mapping to
/// the correct content type for UEFI.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Type {
    /// An EFI module
    #[serde(rename = "application/vnd.dispatch+efi")]
    Efi,

    /// An ISO image
    #[serde(rename = "application/vnd.dispatch+iso")]
    Iso,

    /// A ramdisk image
    #[serde(rename = "application/vnd.dispatch+img")]
    Img,
}

impl Type {
    /// The content type required by UEFI
    pub const fn content_type(&self) -> &str {
        match self {
            Self::Efi => "application/efi",
            Self::Iso => "application/vnd.efi-iso",
            Self::Img => "application/vnd.efi-img",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(untagged)]
enum Knowable<K, U> {
    Known(K),
    Unknown(U),
}

impl<K, U> Knowable<K, U> {
    fn known(self) -> Option<K> {
        match self {
            Self::Known(known) => Some(known),
            Self::Unknown(..) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub struct Asset<T = Type> {
    pub name: String,
    pub size: u64,

    #[serde(rename = "browser_download_url")]
    pub url: String,

    #[serde(rename = "content_type")]
    pub mime: T,
}

impl Asset<Knowable<Type, String>> {
    fn known(self) -> Option<Asset> {
        self.mime.known().map(|mime| Asset {
            name: self.name,
            size: self.size,
            url: self.url,
            mime,
        })
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    assets: Vec<Asset<Knowable<Type, String>>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    title: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    assignees: Option<Vec<String>>,
}

#[derive(Debug, Clone, clap::Args)]
pub struct GitHub {
    /// GitHub token for API access
    #[arg(long, env = "GITHUB_TOKEN")]
    pub token: Option<String>,

    /// GitHub repository owner
    #[arg(short = 'o', long)]
    pub owner: String,

    /// GitHub repository name  
    #[arg(short = 'r', long)]
    pub repo: String,

    /// Release tag to download assets from
    #[arg(short = 't', long)]
    pub tag: String,

    /// Filter asset names
    #[arg(trailing_var_arg = true)]
    pub filter: Vec<String>,
}

impl GitHub {
    const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

    /// Authenticate with GitHub by guiding user to create a Personal Access Token
    pub fn login(&mut self) -> Result<()> {
        // Try to get token from GitHub CLI
        if self.token.is_none() {
            self.token = Command::new("gh")
                .arg("auth")
                .arg("token")
                .output()
                .ok()
                .and_then(|output| {
                    if !output.status.success() {
                        return None;
                    }

                    String::from_utf8(output.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                });
        }

        // If we already have a token, nothing to do
        if self.token.is_some() {
            return Ok(());
        }

        println!("No GitHub token found. Please authenticate with GitHub.");
        println!();
        println!("Option 1");
        println!("  Run 'gh auth login' and choose to authenticate via the web browser flow.");
        println!("  Re-run dispatch.");
        println!();
        println!("Option 2");
        println!("  Run 'gh auth login' and choose to authenticate by pasting an authentication token.");
        println!("    Fine-grained authentication token (PAT)");
        println!("      If using a fine-grained PAT, it must have, at a minimum, the following permissions:");
        println!("        Contents: Read-only access");
        println!("        Issues: Read and write access");
        println!("    Classic PAT");
        println!("      If using a classic PAT, it must have, at a minimum, the following permissions:");
        println!("        repo");
        println!("  Re-run dispatch.");
        println!();
        println!("Option 3");
        println!("  Use the dispatch --token command line option and specify either a fine-grained or a ");
        println!("  classic PAT as described above.");
        println!();
        println!("Option 4");
        println!("  Set an environment variable named GITHUB_TOKEN to either a fine-grained or a classic");
        println!("  PAT as described above.");
        println!("  Re-run dispatch.");
        println!();

        anyhow::bail!("GitHub authentication required. Please follow the instructions above.");
    }

    fn client(&self) -> Client {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Accept", "application/vnd.github+json".parse().unwrap());
        headers.insert("User-Agent", Self::USER_AGENT.parse().unwrap());

        if let Some(token) = &self.token {
            let auth_value = format!("Bearer {token}");
            headers.insert("Authorization", auth_value.parse().unwrap());
        }

        Client::builder()
            .default_headers(headers)
            .build()
            .expect("Failed to create HTTP client")
    }

    pub async fn assets(&self) -> Result<BTreeSet<Asset>> {
        let client = self.client();
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/tags/{}",
            self.owner, self.repo, self.tag
        );

        let response = client.get(&url).send().await?;
        let release: Release = response.json().await?;

        let assets = release
            .assets
            .into_iter()
            .filter_map(Asset::known)
            .filter(|asset| {
                self.filter.is_empty() || self.filter.iter().any(|f| asset.name.contains(f))
            })
            .collect::<BTreeSet<_>>();

        Ok(assets)
    }

    pub async fn report(&self, report: Report) -> Result<()> {
        let client = self.client();
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues",
            self.owner, self.repo
        );

        client.post(&url).json(&report).send().await?;
        Ok(())
    }
}
