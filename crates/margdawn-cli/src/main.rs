//! margdawn — a minimal Markdown viewer for the terminal.

use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;

use margdawn_cli::{ViewOptions, render_to_stdout};
use margdawn_core::config::{self, Config};
use margdawn_core::md::parse;

const HELP: &str = "\
margdawn — minimal Markdown viewer (terminal)

USAGE:
    margdawn [OPTIONS] <file.md>

OPTIONS:
    -t, --theme <NAME>      Override the theme for this run
        --set-theme <NAME>  Persist the theme to the config file and exit
        --import-theme <FILE> [NAME]
                            Import a VS Code color theme (.json) or margdawn
                            theme; NAME defaults to the file stem
        --import-code-theme <FILE> [NAME]
                            Import a syntect .tmTheme for code blocks
        --list-themes       List user-installed and built-in themes
    -a, --ascii             Disable ANSI styling (auto-off when piped)
    -h, --help              Show this help
    -V, --version           Show version

CONFIG:
    ~/.config/margdawn/config.json   { \"theme\": \"monokai-night\" }
    ~/.config/margdawn/themes/...    user themes
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprint!("{HELP}");
        exit(2);
    }

    let mut theme_override: Option<String> = None;
    let mut set_theme: Option<String> = None;
    let mut import: Option<String> = None;
    let mut import_name: Option<String> = None;
    let mut import_code: Option<String> = None;
    let mut import_code_name: Option<String> = None;
    let mut list_themes = false;
    let mut ascii = false;
    let mut files: Vec<PathBuf> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                exit(0);
            }
            "-V" | "--version" => {
                println!("margdawn {}", env!("CARGO_PKG_VERSION"));
                exit(0);
            }
            "-t" | "--theme" => {
                theme_override = args.get(i + 1).map(String::from);
                if theme_override.is_none() {
                    eprintln!("error: --theme requires a name");
                    exit(2);
                }
                i += 1;
            }
            "--set-theme" => {
                set_theme = args.get(i + 1).map(String::from);
                if set_theme.is_none() {
                    eprintln!("error: --set-theme requires a name");
                    exit(2);
                }
                i += 1;
            }
            "--import-theme" => {
                import = args.get(i + 1).map(String::from);
                if import.is_none() {
                    eprintln!("error: --import-theme requires a file");
                    exit(2);
                }
                if let Some(n) = args.get(i + 2) {
                    import_name = Some(n.clone());
                    i += 1;
                }
                i += 1;
            }
            "--import-code-theme" => {
                import_code = args.get(i + 1).map(String::from);
                if import_code.is_none() {
                    eprintln!("error: --import-code-theme requires a file");
                    exit(2);
                }
                if let Some(n) = args.get(i + 2) {
                    import_code_name = Some(n.clone());
                    i += 1;
                }
                i += 1;
            }
            "--list-themes" => list_themes = true,
            "-a" | "--ascii" | "--no-color" => ascii = true,
            other => files.push(PathBuf::from(other)),
        }
        i += 1;
    }

    if set_theme.is_some() {
        match config::set_theme(set_theme.as_deref().unwrap()) {
            Ok(warnings) => {
                println!("theme set to `{}` in {}", set_theme.unwrap(), config::config_path().display());
                for w in warnings {
                    eprintln!("warning: {w}");
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                exit(1);
            }
        }
        return;
    }

    if let Some(file) = import {
        let name = import_name.unwrap_or_else(|| {
            PathBuf::from(&file)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("imported")
                .to_string()
        });
        match margdawn_core::importer::import(std::path::Path::new(&file), &name) {
            Ok(dir) => println!("imported theme `{name}` into {}", dir.display()),
            Err(e) => {
                eprintln!("error: cannot import `{file}`: {e}");
                exit(1);
            }
        }
        return;
    }

    if let Some(file) = import_code {
        let name = import_code_name.unwrap_or_else(|| {
            PathBuf::from(&file)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("code-theme")
                .to_string()
        });
        match margdawn_core::syn::import_code_theme(std::path::Path::new(&file), Some(&name)) {
            Ok(n) => {
                println!("imported code theme `{n}` into {}",
                         config::code_themes_dir().display());
            }
            Err(e) => {
                eprintln!("error: cannot import `{file}`: {e}");
                exit(1);
            }
        }
        return;
    }

    if list_themes {
        let (user, builtin) = config::available_themes();
        println!("user themes:");
        if user.is_empty() {
            println!("  (none — try: margdawn --import-theme <file.json> <name>)");
        } else {
            for t in &user {
                println!("  {t}");
            }
        }
        println!("built-in themes:");
        for t in &builtin {
            if user.iter().any(|u| u == t) {
                println!("  {t}  (overridden by user theme)");
            } else {
                println!("  {t}");
            }
        }
        println!("code themes (syntax highlighting):");
        for t in margdawn_core::syn::code_theme_names() {
            println!("  {t}");
        }
        return;
    }

    if files.is_empty() {
        eprint!("{HELP}");
        exit(2);
    }
    if files.len() > 1 {
        eprintln!("error: margdawn takes one file (got {})", files.len());
        exit(2);
    }
    let path = &files[0];

    let resolved = config::resolve_theme(theme_override.as_deref());
    for w in &resolved.warnings {
        eprintln!("warning: {w}");
    }

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", path.display());
            exit(1);
        }
    };

    let doc = parse(&src);
    let cfg: Config = resolved.config.clone();
    let mut opts = ViewOptions::from_env();
    if ascii {
        opts.ascii = true;
    }
    opts.preserve_blank_lines = cfg.preserve_blank_lines();
    opts.background = cfg.background_enabled();
    opts.code_background = cfg.code_background_enabled();
    let code_theme = cfg.code_theme_name().to_string();
    let status = render_to_stdout(&doc, Arc::clone(&resolved.theme), &code_theme, &opts);
    if let Err(e) = status {
        // EPIPE = pager/pipe closed; exit quietly
        if !matches!(e.kind(), std::io::ErrorKind::BrokenPipe) {
            eprintln!("error: {e}");
            exit(1);
        }
    }
}
