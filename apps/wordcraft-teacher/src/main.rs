#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod dashboard;
mod ru;
mod server;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use wordcraft_classroom::{DEFAULT_PORT, PROTOCOL_VERSION};

use crate::server::{AppState, Classroom};

struct Options {
    port: u16,
    admin_port: u16,
    web_dir: Option<PathBuf>,
    no_open: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let options = match parse_args() {
        Ok(Some(options)) => options,
        Ok(None) => return Ok(()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(run(options))
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut options = Options { port: DEFAULT_PORT, admin_port: server::default_admin_port(), web_dir: None, no_open: false };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => options.port = parse_port(args.next(), "--port")?,
            "--admin-port" => options.admin_port = parse_port(args.next(), "--admin-port")?,
            "--web-dir" => {
                let Some(path) = args.next() else { return Err("--web-dir needs a path".into()) };
                options.web_dir = Some(PathBuf::from(path));
            }
            "--no-open" => options.no_open = true,
            "--version" | "-V" => {
                println!("wordcraft-teacher {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                print_help();
                return Ok(None);
            }
            other => return Err(format!("unknown argument: {other}\n\n{}", help())),
        }
    }
    Ok(Some(options))
}

fn parse_port(value: Option<String>, name: &str) -> Result<u16, String> {
    let Some(value) = value else { return Err(format!("{name} needs a port")) };
    match value.parse::<u16>() {
        Ok(0) | Err(_) => Err(format!("{name} must be a port from 1 to 65535")),
        Ok(port) => Ok(port),
    }
}

fn help() -> String {
    "Usage: wordcraft-teacher [--port 8770] [--admin-port 8771] [--web-dir PATH] [--no-open]\n\n  --port        port for students (default 8770)\n  --admin-port  loopback port for this dashboard (default 8771)\n  --web-dir     folder holding the built WordCraft web app\n  --no-open     do not open the dashboard in a browser\n  --help        show this help\n  --version     print the version".into()
}

fn print_help() {
    println!("{}", help());
}

async fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let web_dir = find_web_dir(options.web_dir);
    let lan_urls = lan_urls(options.port);
    let token = server::random_token();
    let state = AppState {
        classroom: Arc::new(Mutex::new(Classroom::default())),
        token: Arc::from(token.as_str()),
        lan_urls: Arc::new(lan_urls.clone()),
        port: options.port,
        web_dir: web_dir.clone(),
    };

    let student = tokio::net::TcpListener::bind(("0.0.0.0", options.port)).await?;
    let admin = tokio::net::TcpListener::bind(("127.0.0.1", options.admin_port)).await?;
    let student = axum::serve(student, server::student_router(state.clone()).into_make_service_with_connect_info::<SocketAddr>());
    let admin = axum::serve(admin, server::admin_router(state));
    let url = server::admin_url(options.admin_port, &token);
    println!("WordCraft Teacher {} (protocol {PROTOCOL_VERSION})", env!("CARGO_PKG_VERSION"));
    for address in &lan_urls {
        println!("  для учеников: {address}");
    }
    if lan_urls.is_empty() {
        println!("  для учеников: адрес в локальной сети не найден");
    }
    if let Some(dir) = &web_dir {
        println!("  web: {}", dir.display());
    } else {
        println!("  web: папка web не найдена; ученики не смогут загрузить WordCraft");
    }
    println!("  панель: {url}");
    if !options.no_open
        && let Err(error) = open::that(&url)
    {
        log::warn!("cannot open the browser: {error}");
    }
    tokio::try_join!(student, admin)?;
    Ok(())
}

fn find_web_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.push(path);
    }
    if let Some(path) = std::env::var_os("WORDCRAFT_WEB_DIR") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        candidates.push(parent.join("web"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("dist/web"));
        candidates.push(cwd.join("../../dist/web"));
    }
    candidates.into_iter().find(|path| path.join("index.html").is_file())
}

fn lan_urls(port: u16) -> Vec<String> {
    let mut urls = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if let IpAddr::V4(ip) = interface.ip()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
            {
                let url = format!("http://{ip}:{port}");
                if !urls.contains(&url) {
                    urls.push(url);
                }
            }
        }
    }
    urls
}
