use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rh")]
#[command(author, version, about = "A CLI tool for managing Rancher Desktop")]
#[command(propagate_version = true)]
pub struct Cli {
    /// Accept invalid SSL certificates
    #[arg(long, global = true)]
    pub insecure: bool,

    /// Path to rd-engine.json (for API commands)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    /// Increase verbosity
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Timeout for API requests in seconds (default: 30)
    #[arg(long, global = true, env = "RH_TIMEOUT", default_value = "30")]
    pub timeout: u64,

    /// Timeout for file downloads in seconds (default: 600)
    #[arg(
        long,
        global = true,
        env = "RH_DOWNLOAD_TIMEOUT",
        default_value = "600"
    )]
    pub download_timeout: u64,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage k3s cache
    Cache {
        #[command(subcommand)]
        command: CacheCommands,
    },

    /// Run comprehensive diagnostic checks
    Diagnose,

    /// Interact with Rancher Desktop HTTP API
    Api {
        /// API endpoint (e.g., /v1/settings)
        endpoint: String,

        /// HTTP method
        #[arg(short, long, default_value = "GET")]
        method: HttpMethod,

        /// Request body (JSON)
        #[arg(short, long)]
        body: Option<String>,

        /// Read request body from file
        #[arg(short, long)]
        input: Option<PathBuf>,

        /// Output raw response without formatting
        #[arg(long)]
        raw: bool,
    },

    /// Check SSL certificates for required domains
    Certs {
        #[command(subcommand)]
        command: CertsCommands,
    },

    /// Display version information
    Version,

    /// Start the Rancher Desktop backend
    Start,

    /// Stop the Rancher Desktop backend
    Stop,

    /// Restart the Rancher Desktop backend
    Restart,

    /// Show backend status
    Status,

    /// View and modify Rancher Desktop settings
    Settings {
        #[command(subcommand)]
        command: Option<SettingsCommands>,
    },
}

#[derive(Subcommand)]
pub enum CacheCommands {
    /// List cached k3s versions
    List,

    /// Download k3s files to local cache
    Populate {
        /// k3s version to download (e.g., v1.33.3+k3s1)
        version: String,
    },
}

#[derive(Subcommand)]
pub enum CertsCommands {
    /// Test SSL connectivity to required domains
    Check,
}

#[derive(Subcommand)]
pub enum SettingsCommands {
    /// Get a specific setting value
    Get {
        /// Setting path using dot notation (e.g., kubernetes.version)
        path: String,
    },

    /// Set a setting value
    Set {
        /// Setting path using dot notation (e.g., kubernetes.version)
        path: String,

        /// Value to set
        value: String,
    },

    /// Reset all settings to defaults (factory reset)
    Reset,
}

#[derive(Clone, ValueEnum)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpMethod::Get => write!(f, "GET"),
            HttpMethod::Post => write!(f, "POST"),
            HttpMethod::Put => write!(f, "PUT"),
            HttpMethod::Delete => write!(f, "DELETE"),
        }
    }
}
