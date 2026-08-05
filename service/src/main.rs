//! Entry point: `ahara-collector run --config <json> --token-file <path>
//! --credentials <path>`. Topology comes from the Nix-rendered config
//! document; the API token and device credentials are host state passed via
//! systemd credentials. Modules run as threads; a module that cannot start
//! (missing credentials) idles without taking the others down.

use ahara_collector::api::{Api, ModuleFlags};
use ahara_collector::config::{Config, Credentials};
use ahara_collector::metrics::Metrics;
use ahara_collector::registry::Registry;
use ahara_collector::spool::Spool;
use ahara_collector::{api, kasa, sensors, ssdp};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn usage() -> ! {
    eprintln!(
        "usage: ahara-collector run --config <config.json> --token-file <path> --credentials <path>"
    );
    std::process::exit(2);
}

fn read_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("run") {
        usage();
    }
    let config_path = read_arg(&args, "--config").unwrap_or_else(|| usage());
    let token_path = read_arg(&args, "--token-file").unwrap_or_else(|| usage());
    let credentials_path = read_arg(&args, "--credentials").unwrap_or_else(|| usage());

    let config_text = std::fs::read_to_string(&config_path).unwrap_or_else(|e| {
        eprintln!("event=startup_failed reason=config_unreadable path={config_path} error={e}");
        std::process::exit(1);
    });
    let config = Config::from_json(&config_text).unwrap_or_else(|e| {
        eprintln!("event=startup_failed reason=config_invalid error={e}");
        std::process::exit(1);
    });

    let token = std::fs::read_to_string(&token_path)
        .map(|t| t.trim().as_bytes().to_vec())
        .unwrap_or_else(|e| {
            eprintln!("event=startup_failed reason=token_unreadable path={token_path} error={e}");
            std::process::exit(1);
        });
    if token.len() < 16 {
        eprintln!("event=startup_failed reason=token_too_short");
        std::process::exit(1);
    }

    let credentials_text = std::fs::read_to_string(&credentials_path).unwrap_or_default();
    let credentials = Credentials::from_json(&credentials_text).unwrap_or_else(|e| {
        eprintln!("event=startup_failed reason=credentials_invalid error={e}");
        std::process::exit(1);
    });

    let spool = Arc::new(
        Spool::open(
            Path::new(&config.spool.dir),
            config.spool.segment_bytes,
            config.spool.max_bytes,
        )
        .unwrap_or_else(|e| {
            eprintln!(
                "event=startup_failed reason=spool_unavailable dir={} error={e}",
                config.spool.dir
            );
            std::process::exit(1);
        }),
    );
    let metrics = Arc::new(Metrics::default());
    let registry = Arc::new(Registry::default());
    let stop = Arc::new(AtomicBool::new(false));

    if config.airwave_ssdp.enable {
        let relay = ssdp::Relay {
            cfg: config.airwave_ssdp.clone(),
            home_cidr: config.home_cidr,
            home_broadcast: config.home_broadcast,
            bind_address: config.bind_address,
            metrics: Arc::clone(&metrics),
        };
        match ssdp::run(relay, Arc::clone(&stop)) {
            Ok(_handles) => eprintln!("event=module_started module=airwave_ssdp"),
            Err(e) => {
                eprintln!("event=startup_failed reason=ssdp_bind error={e}");
                std::process::exit(1);
            }
        }
    }

    if config.env_sensors.enable {
        let module = sensors::EnvSensorModule {
            cfg: config.env_sensors.clone(),
            creds: credentials.env_sensors.clone(),
            bind_address: config.bind_address,
            home_broadcast: config.home_broadcast,
            spool: Arc::clone(&spool),
            metrics: Arc::clone(&metrics),
            registry: Arc::clone(&registry),
        };
        module.spawn(Arc::clone(&stop));
        eprintln!("event=module_started module=env_sensors");
    }

    if config.kasa.enable {
        let module = kasa::KasaModule {
            cfg: config.kasa.clone(),
            creds: credentials.kasa.clone(),
            bind_address: config.bind_address,
            home_broadcast: config.home_broadcast,
            spool: Arc::clone(&spool),
            metrics: Arc::clone(&metrics),
            registry: Arc::clone(&registry),
        };
        module.spawn(Arc::clone(&stop));
        eprintln!("event=module_started module=kasa");
    }

    let api = Arc::new(Api {
        token,
        ingest_creds: credentials.env_sensors.clone(),
        spool,
        metrics,
        registry,
        modules: ModuleFlags {
            airwave_ssdp: config.airwave_ssdp.enable,
            env_sensors: config.env_sensors.enable && credentials.env_sensors.is_some(),
            kasa: config.kasa.enable && credentials.kasa.is_some(),
        },
    });

    // The API accept loop owns the main thread; systemd stops the whole
    // process with SIGTERM. The spool is crash-safe by construction, so
    // abrupt termination loses at most one torn line.
    if let Err(e) = api::run(api, &config, stop) {
        eprintln!("event=startup_failed reason=api_bind error={e}");
        std::process::exit(1);
    }
}
