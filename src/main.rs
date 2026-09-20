use std::sync::Arc;

use openstory_bot::config::Config;
use openstory_bot::runtime::{self, RunOutcome};
use openstory_bot::state::BotState;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, Mutex};

#[tokio::main]
async fn main() {
    openstory_bot::setup_console_utf8();
    openstory_bot::packet::init_time_baseline();
    let args: Vec<String> = std::env::args().skip(1).collect();

    let config = match Config::parse(&args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    // --config 链生效: 后续 RuntimeConfig 加载/回写全部指向这些文件
    // (最后一个文件为写入目标)。留空 = 传统的 ./config.json。
    if !config.config_paths.is_empty() {
        openstory_bot::runtime_config::set_config_paths(
            config
                .config_paths
                .iter()
                .map(std::path::PathBuf::from)
                .collect(),
        );
    }

    let state = Arc::new(Mutex::new(BotState::default()));

    // Stdin command channel.
    let (tx, mut rx) = mpsc::channel::<String>(64);
    tokio::spawn(async move {
        let stdin = BufReader::new(tokio::io::stdin());
        let mut lines = stdin.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });

    let code = match runtime::run(config, &mut rx, state).await {
        RunOutcome::Failed | RunOutcome::ConnectFailed => 1,
        _ => 0,
    };
    std::process::exit(code);
}

