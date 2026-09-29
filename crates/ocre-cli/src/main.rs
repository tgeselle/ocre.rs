//! `ocre`: command-line tool for Ocre apps.
//!
//! Every command is non-interactive. With `--json`, stdout carries exactly one
//! JSON object (`{"ok": true, ...}` or `{"ok": false, "error", "hint"}`) and
//! all tool output (wrangler, cargo) goes to stderr.

mod generate;
mod names;
mod new;
mod output;
mod project;
mod wrangler;

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};

use output::{CliError, Report};

#[derive(Parser)]
#[command(
    name = "ocre",
    version,
    about = "Ocre: Rails-like Rust web framework for Cloudflare Workers, free plan first."
)]
struct Cli {
    /// Print one JSON object on stdout; send tool logs to stderr.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new app in ./<NAME>.
    New {
        /// App name: lowercase letters, digits and dashes (e.g. `my-blog`).
        name: String,
        /// Use a local checkout of the `ocre` crate instead of the git dependency.
        #[arg(long)]
        ocre_path: Option<PathBuf>,
    },
    /// Generate code (alias: `g`).
    #[command(alias = "g", subcommand)]
    Generate(GenerateCommand),
    /// Apply D1 migrations (local database unless --remote).
    Migrate {
        /// Apply to the production database on Cloudflare.
        #[arg(long)]
        remote: bool,
    },
    /// Apply local migrations, then run the app with `wrangler dev`.
    Dev {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
    /// Deploy to Cloudflare and apply remote migrations.
    Deploy,
}

#[derive(Subcommand)]
enum GenerateCommand {
    /// CRUD resource: migration, model, handlers, routes and templates.
    ///
    /// Example: `ocre g scaffold Post title:string body:text published:boolean`.
    /// Field types: string, text, integer, float, boolean.
    Scaffold {
        /// Singular model name, PascalCase or snake_case (e.g. `BlogPost`).
        name: String,
        /// Fields as `name:type`.
        #[arg(required = true)]
        fields: Vec<String>,
    },
    /// Empty numbered SQL migration file.
    Migration {
        /// Migration name in snake_case (e.g. `add_slug_to_posts`).
        name: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    let result = match cli.command {
        Command::New { name, ocre_path } => new::run(&name, ocre_path.as_deref()),
        Command::Generate(GenerateCommand::Scaffold { name, fields }) => generate::scaffold(&name, &fields),
        Command::Generate(GenerateCommand::Migration { name }) => generate::migration(&name),
        Command::Migrate { remote } => wrangler::migrate(remote, json),
        Command::Dev { port } => wrangler::dev(port, json),
        Command::Deploy => wrangler::deploy(json),
    };
    output::finish(result, json)
}

/// Shorthand used by every command.
pub type CliResult = Result<Report, CliError>;
