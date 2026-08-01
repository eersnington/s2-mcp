use std::{
    fs::File,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{
    CommandFactory, Parser, Subcommand,
    builder::styling::{AnsiColor, Effects, Styles},
};
use s2_mcp::{
    DevSource, Error, LaunchIntent, Policy, ResolvedRuntime, Result, ServerMode, ServerOptions,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "s2-mcp",
    version,
    about = "MCP server for S2 durable streams",
    styles = cli_styles(),
    after_long_help = "CONNECTIONS
  s2-mcp                            Connect to S2 Cloud
  s2-mcp --dev                      Start a temporary S2 Lite container when needed
  s2-mcp --dev --endpoint <URL>     Use an existing S2 development server
  s2-mcp --dev --from-env           Read endpoints from environment variables

MCP CLIENT CONFIGURATION
  Cloud:       command = \"s2-mcp\"
  Development: command = \"s2-mcp\", args = [\"--dev\"]

Use this command with an MCP client. Running it directly in a terminal shows this help."
)]
struct Cli {
    /// Choose how S2 operations are exposed
    #[arg(long, value_enum, default_value_t)]
    mode: ServerMode,

    /// Hide operations that mutate S2 state
    #[arg(long)]
    readonly: bool,

    /// Limit access to one S2 basin
    #[arg(long)]
    basin: Option<String>,

    /// Expose destructive operations; ignored with --readonly
    #[arg(long)]
    allow_destructive: bool,

    /// Write diagnostic logs to a file
    #[arg(long, value_name = "PATH")]
    log_file: Option<PathBuf>,

    /// Use an isolated development connection
    #[arg(long)]
    dev: bool,

    /// Connect to an existing development endpoint
    #[arg(
        long,
        value_name = "URL",
        requires = "dev",
        conflicts_with = "from_env"
    )]
    endpoint: Option<String>,

    /// Read account and basin endpoints from environment variables
    #[arg(long, requires = "dev", conflicts_with = "endpoint")]
    from_env: bool,

    #[command(subcommand)]
    command: Option<InternalCommand>,
}

const fn cli_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
        .usage(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
        .literal(AnsiColor::Green.on_default().effects(Effects::BOLD))
        .placeholder(AnsiColor::Yellow.on_default())
        .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
        .valid(AnsiColor::Green.on_default())
        .invalid(AnsiColor::Yellow.on_default())
}

#[derive(Debug, Subcommand)]
enum InternalCommand {
    #[command(name = "__execute", hide = true)]
    Execute,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if should_show_interactive_guidance(cli.command.as_ref(), io::stdin().is_terminal()) {
        if let Err(error) = print_interactive_help() {
            eprintln!("failed to print help: {error}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }
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
        dev,
        endpoint,
        from_env,
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
        None => run_server(options, launch_intent(dev, endpoint, from_env)).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn print_interactive_help() -> io::Result<()> {
    let mut command = Cli::command().after_long_help(None::<&str>);
    command.print_long_help()?;

    let styles = cli_styles();
    let heading = styles.get_header();
    let command = styles.get_literal();
    let placeholder = styles.get_placeholder();
    let mut stdout = io::stdout().lock();
    writeln!(stdout)?;
    writeln!(
        stdout,
        "{}CONNECTIONS{}",
        heading.render(),
        heading.render_reset()
    )?;
    writeln!(
        stdout,
        "  {}s2-mcp{}                            Connect to S2 Cloud",
        command.render(),
        command.render_reset()
    )?;
    writeln!(
        stdout,
        "  {}s2-mcp --dev{}                      Start a temporary S2 Lite container when needed",
        command.render(),
        command.render_reset()
    )?;
    writeln!(
        stdout,
        "  {}s2-mcp --dev --endpoint{} {}<URL>{}     Use an existing S2 development server",
        command.render(),
        command.render_reset(),
        placeholder.render(),
        placeholder.render_reset()
    )?;
    writeln!(
        stdout,
        "  {}s2-mcp --dev --from-env{}           Read endpoints from environment variables",
        command.render(),
        command.render_reset()
    )?;
    writeln!(stdout)?;
    writeln!(
        stdout,
        "{}MCP CLIENT CONFIGURATION{}",
        heading.render(),
        heading.render_reset()
    )?;
    writeln!(
        stdout,
        "  {}Cloud:{}       command = {}\"s2-mcp\"{}",
        heading.render(),
        heading.render_reset(),
        command.render(),
        command.render_reset()
    )?;
    writeln!(
        stdout,
        "  {}Development:{} command = {}\"s2-mcp\", args = [\"--dev\"]{}",
        heading.render(),
        heading.render_reset(),
        command.render(),
        command.render_reset()
    )?;
    writeln!(stdout)?;
    writeln!(
        stdout,
        "Use this command with an MCP client. Running it directly in a terminal shows this help."
    )
}

fn should_show_interactive_guidance(
    command: Option<&InternalCommand>,
    stdin_is_terminal: bool,
) -> bool {
    command.is_none() && stdin_is_terminal
}

fn launch_intent(dev: bool, endpoint: Option<String>, from_env: bool) -> LaunchIntent {
    if !dev {
        LaunchIntent::Cloud
    } else if let Some(endpoint) = endpoint {
        LaunchIntent::Dev(DevSource::Endpoint(endpoint))
    } else if from_env {
        LaunchIntent::Dev(DevSource::Environment)
    } else {
        LaunchIntent::Dev(DevSource::Managed)
    }
}

async fn run_server(options: ServerOptions, intent: LaunchIntent) -> Result<()> {
    let runtime = ResolvedRuntime::resolve(intent)?;
    s2_mcp::serve_runtime(options, runtime).await
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
