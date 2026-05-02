use clap::{CommandFactory, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(name = "bitchain", about = "A Rust CLI with JSON config support.", long_about = None)]
struct Cli {
    /// Create or update the config file
    #[arg(long)]
    setup_config: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show help information for the CLI
    Help,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Config {
    aws: Option<AwsCredentials>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AwsCredentials {
    access_key_id: String,
    secret_access_key: String,
}

fn main() {
    let config_path = config_file_path();
    let config = load_config(&config_path);

    let cli = Cli::parse();

    if let Some(c) = config {
        println!("Loaded config from {}", config_path.display());
        println!("Current config: {:#?}", c);
    } else if config_path.exists() {
        eprintln!("Failed to load config from {}", config_path.display());
    }

    if cli.setup_config {
        if let Err(err) = setup_config(&config_path) {
            eprintln!("Failed to setup config: {err}");
            std::process::exit(1);
        }
        return;
    }

    match &cli.command {
        Some(Commands::Help) => {
            let mut cmd = Cli::command();
            cmd.print_help().expect("Failed to print help");
            println!();
        }
        None => {
            println!("Run `bitchain help` or `bitchain --help` for available commands.");
        }
    }
}

fn config_file_path() -> PathBuf {
    let home = env::var("HOME").expect("HOME environment variable is not set");
    Path::new(&home).join(".bitchain").join("config")
}

fn load_config(config_path: &Path) -> Option<Config> {
    if !config_path.exists() {
        return None;
    }

    let contents = fs::read_to_string(config_path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn setup_config(config_path: &Path) -> io::Result<()> {
    println!("Setting up config file at {}", config_path.display());

    let mut config = Config::default();

    if confirm("Would you like to add AWS credentials to the config? (y/N)")? {
        let access_key_id = prompt("AWS Access Key ID")?;
        let secret_access_key = prompt("AWS Secret Access Key")?;

        config.aws = Some(AwsCredentials {
            access_key_id,
            secret_access_key,
        });
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(&config).unwrap();
    fs::write(config_path, json)?;

    println!("Config saved to {}", config_path.display());
    Ok(())
}

fn confirm(prompt_text: &str) -> io::Result<bool> {
    let answer = prompt(prompt_text)?;
    let normalized = answer.trim().to_lowercase();
    Ok(normalized == "y" || normalized == "yes")
}

fn prompt(field: &str) -> io::Result<String> {
    print!("{}: ", field);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_owned())
}
