use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::config::Credentials;
use aws_sdk_s3::primitives::ByteStream;
use clap::{CommandFactory, Parser, Subcommand};
use reqwest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use tokio;

const DEFAULT_BLOCK_SIZE: usize = 1024 * 1024;

#[derive(Parser, Debug)]
#[command(name = "bitchain", about = "Manage bitchains: JSON objects listing URIs to binary blocks.", long_about = None, disable_help_subcommand = true)]
struct Cli {
    /// Create or update the config file
    #[arg(long)]
    setup_config: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Ingest a file or directory and produce a bitchain JSON object containing block URIs
    Ingest {
        /// Input file or directory to break into blocks
        #[arg(short, long)]
        input: PathBuf,

        /// Directory to store block files locally
        #[arg(long)]
        output_dir: Option<PathBuf>,

        /// Block size in bytes for splitting input data
        #[arg(long, default_value_t = DEFAULT_BLOCK_SIZE)]
        block_size: usize,

        /// Base URI to use for generated block references
        #[arg(long)]
        uri_base: Option<String>,

        /// Output bitchain JSON file path
        #[arg(long)]
        output: Option<PathBuf>,

        /// Do not write files or upload objects; only simulate the workflow
        #[arg(long)]
        dry_run: bool,
    },
    /// Rebuild files from an existing bitchain JSON file
    Rebuild {
        /// Bitchain JSON file to read
        #[arg(long)]
        bitchain: PathBuf,

        /// Output directory to reconstruct files into
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Print a bitchain JSON file to stdout
    Show {
        /// Bitchain JSON file to show
        file: PathBuf,
    },
    /// Validate a bitchain JSON file structure
    Validate {
        /// Bitchain JSON file to validate
        file: PathBuf,
    },
    /// Print CLI help
    Help,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Config {
    aws: Option<AwsConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AwsConfig {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    region: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Bitchain {
    version: String,
    files: Vec<BitchainFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitchainFile {
    path: String,
    blocks: Vec<Block>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Block {
    hash: String,
    uris: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OldBitchain {
    version: String,
    blocks: Vec<Block>,
}

#[tokio::main]
async fn main() {
    let config_path = config_file_path();
    let cli = Cli::parse();

    if cli.setup_config {
        if let Err(err) = setup_config(&config_path) {
            eprintln!("Failed to setup config: {err}");
            std::process::exit(1);
        }
        return;
    }

    let config = load_config(&config_path);
    if let Some(c) = &config {
        println!("Loaded config from {}", config_path.display());
        println!("Current config: {:#?}", c);
    } else if config_path.exists() {
        eprintln!("Failed to load config from {}", config_path.display());
    }

    match cli.command.unwrap_or(Commands::Help) {
        Commands::Help => {
            let mut cmd = Cli::command();
            cmd.print_help().expect("Failed to print help");
            println!();
        }
        Commands::Ingest {
            input,
            output_dir,
            block_size,
            uri_base,
            output,
            dry_run,
        } => {
            let output_file = output.unwrap_or_else(|| input.with_extension("bitchain.json"));
            if let Err(err) = ingest_path(
                &input,
                block_size,
                output_dir.as_deref(),
                uri_base.as_deref(),
                &output_file,
                &config,
                dry_run,
            )
            .await
            {
                eprintln!("Ingest failed: {err}");
                std::process::exit(1);
            }
        }
        Commands::Rebuild {
            bitchain,
            output_dir,
        } => {
            if let Err(err) = rebuild_bitchain(&bitchain, &output_dir, &config).await {
                eprintln!("Rebuild failed: {err}");
                std::process::exit(1);
            }
        }
        Commands::Show { file } => {
            if let Err(err) = show_bitchain(&file) {
                eprintln!("Show failed: {err}");
                std::process::exit(1);
            }
        }
        Commands::Validate { file } => {
            if let Err(err) = validate_bitchain(&file) {
                eprintln!("Validate failed: {err}");
                std::process::exit(1);
            }
            println!("Bitchain is valid: {}", file.display());
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
        let region = prompt("AWS Region (e.g., us-east-1)")?;

        let session_token = if confirm("Do you have an AWS session token to add? (y/N)")? {
            let token = prompt("AWS Session Token")?;
            if token.is_empty() {
                None
            } else {
                Some(token)
            }
        } else {
            None
        };

        config.aws = Some(AwsConfig {
            access_key_id,
            secret_access_key,
            session_token,
            region,
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

async fn ingest_path(
    input_path: &Path,
    block_size: usize,
    output_dir: Option<&Path>,
    uri_base: Option<&str>,
    output_bitchain: &Path,
    config: &Option<Config>,
    dry_run: bool,
) -> io::Result<()> {
    if let Some(base) = uri_base {
        if base.trim_start_matches(' ').starts_with("s3://")
            && config.as_ref().and_then(|c| c.aws.as_ref()).is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "S3 uri_base requires AWS credentials in config; run --setup-config and provide AWS credentials",
            ));
        }
    }

    let output_dir = output_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().expect("Failed to determine current directory"));
    fs::create_dir_all(&output_dir)?;

    let mut files = Vec::new();
    if input_path.is_file() {
        let relative_path = input_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output.bin")
            .to_string();
        files.push(
            ingest_file_entry(
                input_path,
                &relative_path,
                &output_dir,
                block_size,
                uri_base,
                config,
                dry_run,
            )
            .await?,
        );
    } else if input_path.is_dir() {
        for entry in fs::read_dir(input_path)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let relative_path = path
                .strip_prefix(input_path)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            files.push(
                ingest_file_entry(
                    &path,
                    &relative_path,
                    &output_dir,
                    block_size,
                    uri_base,
                    config,
                    dry_run,
                )
                .await?,
            );
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Input path must be a file or directory",
        ));
    }

    let bitchain = Bitchain {
        version: "1.0".to_string(),
        files,
    };

    if dry_run {
        println!(
            "Dry run complete. Planned {} file(s) into {}.",
            bitchain.files.len(),
            output_bitchain.display()
        );
        return Ok(());
    }

    if let Some(parent) = output_bitchain.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(&bitchain).unwrap();
    fs::write(output_bitchain, json)?;
    println!("Wrote bitchain to {}", output_bitchain.display());
    Ok(())
}

async fn ingest_file_entry(
    file_path: &Path,
    relative_path: &str,
    output_dir: &Path,
    block_size: usize,
    uri_base: Option<&str>,
    config: &Option<Config>,
    dry_run: bool,
) -> io::Result<BitchainFile> {
    let input_file = File::open(file_path)?;
    let mut reader = BufReader::new(input_file);
    let mut blocks = Vec::new();
    let mut buffer = vec![0u8; block_size];

    while let Ok(bytes_read) = reader.read(&mut buffer) {
        if bytes_read == 0 {
            break;
        }

        let data = &buffer[..bytes_read];
        let hash = format!("{:x}", Sha256::digest(data));
        let mut uris = Vec::new();

        if let Some(base) = uri_base {
            let uri = format!(
                "{}/{}/{}",
                base.trim_end_matches('/'),
                relative_path.trim_start_matches('/'),
                hash
            );
            uris.push(uri.clone());

            if uri.starts_with("s3://") {
                if !dry_run {
                    let aws_config = config
                        .as_ref()
                        .and_then(|c| c.aws.as_ref())
                        .ok_or_else(|| io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "S3 uri_base requires AWS credentials in config; run --setup-config and provide AWS credentials",
                        ))?;
                    upload_to_s3(&uri, data, aws_config).await.map_err(|err| {
                        io::Error::new(
                            io::ErrorKind::Other,
                            format!("Failed to upload block {}: {}", uri, err),
                        )
                    })?;
                } else {
                    println!("Dry run: would upload {}", uri);
                }
            } else if dry_run {
                println!("Dry run: would create URI {}", uri);
            }
        } else {
            let file_output_dir = output_dir.join(relative_path).with_extension("blocks");
            fs::create_dir_all(&file_output_dir)?;
            let block_path = file_output_dir.join(format!("{}.bin", hash));
            if !dry_run {
                fs::write(&block_path, data)?;
            }
            let uri = format!("file://{}", fs::canonicalize(&block_path)?.display());
            uris.push(uri);
        }

        blocks.push(Block { hash, uris });
    }

    Ok(BitchainFile {
        path: relative_path.to_string(),
        blocks,
    })
}

async fn rebuild_bitchain(
    bitchain_path: &Path,
    output_dir: &Path,
    config: &Option<Config>,
) -> io::Result<()> {
    let bitchain = load_bitchain(bitchain_path)?;
    fs::create_dir_all(output_dir)?;

    for file_entry in &bitchain.files {
        let reconstructed_path = output_dir.join(&file_entry.path);
        if let Some(parent) = reconstructed_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut output_file = File::create(&reconstructed_path)?;
        for block in &file_entry.blocks {
            let mut block_data = None;
            for uri in &block.uris {
                match download_object(uri, config).await {
                    Ok(data) => {
                        block_data = Some(data);
                        break;
                    }
                    Err(err) => {
                        eprintln!("Failed to download {}: {}", uri, err);
                    }
                }
            }
            let data = block_data.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("All URIs failed for block {}", block.hash),
                )
            })?;
            output_file.write_all(&data)?;
        }
        println!("Rebuilt file {}", reconstructed_path.display());
    }

    Ok(())
}

async fn download_object(uri: &str, config: &Option<Config>) -> io::Result<Vec<u8>> {
    if uri.starts_with("s3://") {
        let aws_config = config
            .as_ref()
            .and_then(|c| c.aws.as_ref())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "S3 download requires AWS credentials in config",
                )
            })?;
        download_from_s3(uri, aws_config).await
    } else if uri.starts_with("http://") || uri.starts_with("https://") {
        download_http(uri).await
    } else if uri.starts_with("file://") {
        let local_path = uri.trim_start_matches("file://").to_string();
        fs::read(local_path)
    } else {
        fs::read(uri)
    }
}

async fn download_http(uri: &str) -> io::Result<Vec<u8>> {
    let response = reqwest::get(uri)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("HTTP request failed: {}", e)))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("HTTP body failed: {}", e)))?;
    if !status.is_success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("HTTP request returned status {}", status),
        ));
    }
    Ok(bytes.to_vec())
}

async fn download_from_s3(uri: &str, aws_config: &AwsConfig) -> io::Result<Vec<u8>> {
    let s3_uri = uri
        .strip_prefix("s3://")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid S3 URI"))?;
    let parts: Vec<&str> = s3_uri.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid S3 URI format",
        ));
    }
    let bucket = parts[0];
    let key = parts[1];

    let shared_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .region(Region::new(aws_config.region.clone()))
        .credentials_provider(Credentials::new(
            &aws_config.access_key_id,
            &aws_config.secret_access_key,
            aws_config.session_token.clone(),
            None,
            "bitchain",
        ))
        .load()
        .await;

    let client = aws_sdk_s3::Client::new(&shared_config);
    let resp = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("S3 download error: {:?}", e)))?;

    let data = resp.body.collect().await.map_err(|e| {
        io::Error::new(io::ErrorKind::Other, format!("S3 body read error: {:?}", e))
    })?;
    Ok(data.into_bytes().to_vec())
}

async fn upload_to_s3(uri: &str, data: &[u8], aws_config: &AwsConfig) -> io::Result<()> {
    let s3_uri = uri
        .strip_prefix("s3://")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid S3 URI"))?;
    let parts: Vec<&str> = s3_uri.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid S3 URI format",
        ));
    }
    let bucket = parts[0];
    let key = parts[1];

    let shared_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .region(Region::new(aws_config.region.clone()))
        .credentials_provider(Credentials::new(
            &aws_config.access_key_id,
            &aws_config.secret_access_key,
            aws_config.session_token.clone(),
            None,
            "bitchain",
        ))
        .load()
        .await;

    let client = aws_sdk_s3::Client::new(&shared_config);
    let body = ByteStream::from(data.to_vec());

    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(body)
        .send()
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("S3 upload error: {:?}", e)))?;

    println!("Uploaded block to {}", uri);
    Ok(())
}

fn show_bitchain(file: &Path) -> io::Result<()> {
    let bitchain = load_bitchain(file)?;
    let json = serde_json::to_string_pretty(&bitchain).unwrap();
    println!("{}", json);
    Ok(())
}

fn validate_bitchain(file: &Path) -> io::Result<()> {
    let bitchain = load_bitchain(file)?;
    if bitchain.files.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Bitchain contains no files",
        ));
    }
    for file_entry in &bitchain.files {
        if file_entry.path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Bitchain file entry has no path",
            ));
        }
        if file_entry.blocks.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("File {} contains no blocks", file_entry.path),
            ));
        }
        for block in &file_entry.blocks {
            if block.uris.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Block {} has no URIs", block.hash),
                ));
            }
        }
    }
    Ok(())
}

fn load_bitchain(file: &Path) -> io::Result<Bitchain> {
    let contents = fs::read_to_string(file)?;
    let raw_value: serde_json::Value = serde_json::from_str(&contents).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse bitchain JSON: {err}"),
        )
    })?;

    if raw_value.get("files").is_some() {
        let bitchain: Bitchain = serde_json::from_value(raw_value).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse new-style bitchain JSON: {err}"),
            )
        })?;
        Ok(bitchain)
    } else if raw_value.get("blocks").is_some() {
        let old_bitchain: OldBitchain = serde_json::from_value(raw_value).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse old-style bitchain JSON: {err}"),
            )
        })?;
        let path = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("restored.bin")
            .to_string();
        Ok(Bitchain {
            version: old_bitchain.version,
            files: vec![BitchainFile {
                path,
                blocks: old_bitchain.blocks,
            }],
        })
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported bitchain schema: missing files or blocks",
        ))
    }
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
