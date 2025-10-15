use std::collections::{BTreeSet, HashMap};
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
pub struct Report<M = String> {
    title: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    assignees: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    milestone: Option<M>,
}

#[derive(Debug, Clone, clap::Args)]
pub struct GitHubArgs {
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

impl GitHubArgs {
    /// Authenticate with GitHub by guiding user to create a Personal Access Token
    pub async fn login(mut self) -> Result<GitHub> {
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

        // If we already have a token, create the client.
        if self.token.is_some() {
            return GitHub::new(self).await;
        }

        println!("No GitHub token found. Please authenticate with GitHub.");
        println!();
        println!("Option 1 - Use GitHub CLI (recommended):");
        println!("  gh auth login");
        println!("  # Re-run this command");
        println!();
        println!("Option 2 - Create a Personal Access Token manually:");
        println!("  1. Visit: https://github.com/settings/tokens/new");
        println!("  2. Select scopes (permissions needed):");
        println!("     • Repository access: Select your target repository");
        println!("     • Repository permissions:");
        println!("       - Contents: Read (for downloading release assets)");
        println!("       - Issues: Write (for creating issues)");
        println!("  3. Click 'Generate token'");
        println!();
        println!("  export GITHUB_TOKEN=<YOUR_TOKEN>");
        println!("  # Re-run this command");
        println!();

        anyhow::bail!("GitHub authentication required. Please follow the instructions above.");
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Milestone {
    title: String,
    number: u64,
}

#[derive(Debug)]
pub struct GitHub {
    args: GitHubArgs,
    client: Client,
    milestones: HashMap<String, u64>,
}

impl GitHub {
    const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
    const PER_PAGE: u32 = 100;

    async fn new(args: GitHubArgs) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Accept", "application/vnd.github+json".parse()?);
        headers.insert("User-Agent", Self::USER_AGENT.parse()?);

        if let Some(token) = &args.token {
            let auth_value = format!("Bearer {token}");
            headers.insert("Authorization", auth_value.parse()?);
        }

        let client = Client::builder().default_headers(headers).build()?;
        let mut milestones = HashMap::new();

        // Load all milestones...
        for n in 1.. {
            let url = format!(
                "https://api.github.com/repos/{}/{}/milestones?state=all&per_page={}&page={}",
                args.owner,
                args.repo,
                Self::PER_PAGE,
                n
            );

            let response = client.get(&url).send().await?;
            let page: Vec<Milestone> = response.json().await?;
            if page.is_empty() {
                break;
            }

            for milestone in page {
                milestones.insert(milestone.title, milestone.number);
            }
        }

        Ok(Self {
            args,
            client,
            milestones,
        })
    }

    pub async fn assets(&self) -> Result<BTreeSet<Asset>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/tags/{}",
            self.args.owner, self.args.repo, self.args.tag
        );

        let response = self.client.get(&url).send().await?;
        let release: Release = response.json().await?;

        let assets = release
            .assets
            .into_iter()
            .filter_map(Asset::known)
            .filter(|asset| {
                self.args.filter.is_empty()
                    || self.args.filter.iter().any(|f| asset.name.contains(f))
            })
            .collect::<BTreeSet<_>>();

        Ok(assets)
    }

    fn milestone(&self, title: &str) -> Result<u64> {
        self.milestones
            .get(title)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Milestone '{}' not found", title))
    }

    pub async fn report(&self, report: Report) -> Result<()> {
        let report = Report {
            title: report.title,
            body: report.body,
            labels: report.labels,
            assignees: report.assignees,
            milestone: report.milestone.map(|t| self.milestone(&t)).transpose()?,
        };

        let url = format!(
            "https://api.github.com/repos/{}/{}/issues",
            self.args.owner, self.args.repo
        );

        self.client.post(&url).json(&report).send().await?;
        Ok(())
    }
}
