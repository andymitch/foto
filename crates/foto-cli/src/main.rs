use std::sync::Arc;

use foto_core::{Config, Event, EventSink, Filter, Foto, NoPlatform};

const USAGE: &str = "usage: foto <command>

  create <name>      create a library
  join <ticket>      join a library from a ticket
  share [--read]     print a ticket for the first library
  add <folder>       watch a folder
  ls                 list assets
  status             sync status, folders and devices
  run                stay online and sync, printing events

env: FOTO_HOME (data dir), FOTO_PICTURES (library folder), RUST_LOG";

struct Print;

impl EventSink for Print {
    fn on_event(&self, event: Event) {
        println!("{event:?}");
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let platform = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let foto = Foto::open(Config::desktop(platform), Arc::new(NoPlatform)).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    if let Err(e) = run(&foto, cmd, &args[1..]) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    foto.shutdown();
}

fn lib(foto: &Foto) -> Result<String, String> {
    foto.libraries()
        .first()
        .map(|l| l.id.clone())
        .ok_or_else(|| "no library yet: run `foto create <name>` or `foto join <ticket>`".into())
}

fn run(foto: &Foto, cmd: &str, rest: &[String]) -> Result<(), String> {
    let arg = || rest.first().cloned().ok_or_else(|| USAGE.to_string());
    let e = |e: foto_core::FotoError| e.to_string();
    match cmd {
        "create" => println!("{}", foto.create_library(arg()?).map_err(e)?.id),
        "join" => println!("{}", foto.join(arg()?).map_err(e)?.id),
        "share" => {
            let writable = rest.first().map(String::as_str) != Some("--read");
            println!("{}", foto.share(lib(foto)?, writable).map_err(e)?);
        }
        "add" => {
            foto.add_root(lib(foto)?, arg()?).map_err(e)?;
            foto.scan_now().map_err(e)?;
        }
        "ls" => {
            let lib = lib(foto)?;
            for a in foto.timeline(lib.clone(), Filter::All) {
                let d = foto.asset(lib.clone(), a.hash.clone());
                let name = d.map(|d| d.name).unwrap_or_default();
                println!("{} {:>6} {:?} {}", &a.hash[..12], a.taken_at, a.kind, name);
            }
        }
        "status" => {
            println!("device {} ({})", foto.device_name(), foto.device_id());
            println!("{:?}", foto.status());
            for r in foto.roots() {
                println!("folder {} files={} online={}", r.path, r.files, r.online);
            }
            for l in foto.libraries() {
                println!("library {} {} writable={}", l.name, l.id, l.writable);
                for d in foto.devices(l.id.clone()) {
                    println!(
                        "  device {} {} online={} held={}/{}",
                        d.name, d.platform, d.online, d.held, d.total
                    );
                }
            }
        }
        "run" => {
            foto.subscribe(Arc::new(Print));
            loop {
                std::thread::park();
            }
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
