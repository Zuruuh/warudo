#![deny(clippy::unwrap_used)]

use clap::Parser;
use futures::future::join;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{Mutex, RwLock},
    task::JoinError,
    time::Instant,
};
use tracing_subscriber::EnvFilter;
use watchexec_events::Event;

mod events;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Arguments {
    #[arg(short = 'r', long, default_value=std::env::current_dir().map(PathBuf::into_os_string).unwrap())]
    root: PathBuf,
    // Where to replicate the `root`
    target: PathBuf,
}

// $ watchexec -nr --emit-events-to=json-stdio --only-emit-events | cargo run
#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_env("WARUDO_LOG"))
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::ENTER)
        .init();

    let args = Arc::new(Arguments::parse());
    tracing::info!("Parsed the following CLI {args:?}");

    if let Some(error) = watch(args).await {
        tracing::error!("One of the task exited ? {error:?}");

        Err(error.into())
    } else {
        Ok(())
    }
}

async fn watch(args: Arc<Arguments>) -> Option<JoinError> {
    let events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let last_event_at = Arc::new(RwLock::new(Instant::now()));

    let stdin_listener_handle = {
        let events_ref = Arc::clone(&events);
        let last_event_at_ref = Arc::clone(&last_event_at);

        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let stdin_reader = BufReader::new(stdin);
            let mut lines = stdin_reader.lines();

            loop {
                let line = lines.next_line().await;
                match line {
                    Ok(Some(line)) => {
                        *last_event_at_ref.write().await = Instant::now();
                        let event = match serde_json::from_str::<Event>(&line) {
                            Ok(event) => event,
                            Err(err) => {
                                tracing::error!("Malformed json input in stdin! Stopping...");
                                tracing::trace!("{line}");
                                tracing::trace!("{err:?}");

                                std::process::exit(1);
                            }
                        };

                        events_ref.lock().await.push(event);
                    }
                    Err(err) => {
                        dbg!(err);
                    }
                    Ok(None) => {
                        println!("Reached end ?");
                        break;
                    }
                }
            }
        })
    };

    let worker_handle = {
        let events_ref = Arc::clone(&events);
        let last_event_at_ref = Arc::clone(&last_event_at);

        tokio::spawn(async move {
            loop {
                let debounce_timeout = last_event_at_ref
                    .read()
                    .await
                    .clone()
                    .checked_add(Duration::from_millis(100))
                    .expect("Duration out of bounds wtf ?");

                if Instant::now() < debounce_timeout {
                    tokio::time::sleep_until(debounce_timeout).await;

                    continue;
                }

                let cloned_events: Vec<Event>;

                {
                    let mut events_handle = events_ref.lock().await;
                    cloned_events = events_handle.clone().to_vec();
                    events_handle.clear();
                }

                if !cloned_events.is_empty() {
                    tracing::trace!("Handling {} event(s)...", cloned_events.len());
                    events::handle_events(cloned_events, args.clone()).await;
                }

                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };

    let (stdin_listener_results, worker_results) = join(stdin_listener_handle, worker_handle).await;

    stdin_listener_results
        .err()
        .or_else(|| worker_results.err())
}
