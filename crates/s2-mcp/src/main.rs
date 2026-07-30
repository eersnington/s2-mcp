use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use s2_mcp::{Error, Policy, Result, S2Configuration, ServerMode, ServerOptions};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "s2-mcp",
    version,
    about = "Local MCP server for S2 durable streams"
)]
struct Cli {
    /// Select the MCP tool exposure mode
    #[arg(long, value_enum, default_value_t)]
    mode: ServerMode,

    /// Hide all S2 operations that can mutate state
    #[arg(long)]
    readonly: bool,

    /// Restrict the server to one S2 basin
    #[arg(long)]
    basin: Option<String>,

    /// Advertise destructive operations; has no effect with --readonly
    #[arg(long)]
    allow_destructive: bool,

    /// Write diagnostic logs to a file
    #[arg(long, value_name = "PATH")]
    log_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<InternalCommand>,
}

#[derive(Debug, Subcommand)]
enum InternalCommand {
    #[command(name = "__execute", hide = true)]
    Execute,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(error) = init_tracing(cli.log_file.as_deref()) {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    let Cli {
        mode,
        readonly,
        basin,
        allow_destructive,
        command,
        ..
    } = cli;
    let options = ServerOptions {
        mode,
        policy: Policy {
            readonly,
            basin,
            allow_destructive,
        },
    };
    let result = match command {
        Some(InternalCommand::Execute) => s2_mcp::run_executor_child().await,
        None => run_server(options).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_server(options: ServerOptions) -> Result<()> {
    let configuration = S2Configuration::load()?;
    s2_mcp::serve(options, configuration).await
}

fn init_tracing(log_file: Option<&Path>) -> Result<()> {
    let environment_filter = EnvFilter::from_default_env();
    if let Some(path) = log_file {
        let file = File::create(path).map_err(|source| Error::CreateLogFile {
            path: path.to_owned(),
            source,
        })?;
        tracing_subscriber::fmt()
            .with_env_filter(environment_filter)
            .with_writer(file)
            .with_ansi(false)
            .try_init()
            .map_err(|source| Error::InitializeLogging { source })
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(environment_filter)
            .with_writer(io::stderr)
            .with_ansi(false)
            .try_init()
            .map_err(|source| Error::InitializeLogging { source })
    }
}
