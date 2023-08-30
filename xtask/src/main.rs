// Copyright 2023 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        mpsc::{self, RecvTimeoutError},
        Arc,
    },
    time::Duration,
};

use anyhow::{Context, Error, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use console::Term;
use indicatif::{HumanDuration, MultiProgress, ProgressBar, ProgressStyle};
use indicatif_log_bridge::LogWrapper;
use log::{error, info};

mod ci;
mod codegen;
mod fmt;

use fmt::{
    CancellationTokenSource, RayonTaskScheduler, TargetGraph, TargetId, TargetSchedulerMessage,
    TaskId,
};

use crate::fmt::{TargetMetadata, TaskMetadata};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// CI related commands.
    Ci(CiCli),

    /// Regenerate source.
    Codegen,

    /// Format source code.
    Fmt(FmtCli),
}

#[derive(Args)]
struct CiCli {
    /// The path to the output json file.
    #[arg(long)]
    output_path: PathBuf,

    /// The left text of the coverage badge.
    #[arg(long)]
    label: String,

    /// The json field name and the path to the json coverage report separated by colon.
    coverage_report: PathBuf,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum FmtFileType {
    /// Format markdown files.
    Markdown,
}

#[derive(Args)]
struct FmtCli {
    /// The file types to format. For example markdown. Default to all.
    #[arg(long, value_enum)]
    file_type: Vec<FmtFileType>,

    /// Do not apply changes to files
    #[arg(long)]
    check: bool,
}

fn main() -> Result<()> {
    let logger = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .filter_module("bindgen", log::LevelFilter::Error)
        .build();
    let multi_progress = MultiProgress::new();
    LogWrapper::new(multi_progress.clone(), logger)
        .try_init()
        .context("initialize logger")?;

    let cli = Cli::parse();
    let res = match &cli.command {
        Commands::Ci(ci_cli) => ci::main(ci_cli),
        Commands::Codegen => codegen::main(),
        Commands::Fmt(_) => Ok(()),
    };
    res?;

    let targets = cli.create_targets();
    let target_graph = TargetGraph::create_from_targets(targets);
    let cancellation_token_source = Arc::<CancellationTokenSource>::default();
    let progress_style = {
        let tick_chars = if Term::stderr().features().wants_emoji() {
            "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
        } else {
            "-\\|/"
        };
        ProgressStyle::with_template(
            "{spinner} [{elapsed_precise}] {prefix:<20!.bold.dim} {wide_msg}",
        )
        .context("create spinner progress style")?
        .tick_chars(tick_chars)
    };
    let (tx, rx) = mpsc::channel();
    let worker_thread = std::thread::Builder::new()
        .name("progress render thread".to_owned())
        .spawn(move || {
            let mut target_progress_bars = BTreeMap::<TargetId, ProgressBar>::default();
            let mut task_progress_bars = BTreeMap::<TaskId, ProgressBar>::default();
            let mut errors: Vec<Error> = vec![];
            loop {
                let message = match rx.recv_timeout(Duration::from_millis(80)) {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {
                        for progress_bar in target_progress_bars.values() {
                            progress_bar.tick();
                        }
                        for progress_bar in task_progress_bars.values() {
                            progress_bar.tick();
                        }
                        continue;
                    }
                };
                fn get_prefix(
                    target_metadata: &TargetMetadata,
                    task_metadata: &TaskMetadata,
                ) -> String {
                    match &task_metadata.name {
                        Some(task_name) => format!("{}: {}", target_metadata.name, task_name),
                        None => target_metadata.name.clone(),
                    }
                }
                match message {
                    TargetSchedulerMessage::TaskScheduled {
                        target_metadata,
                        task_metadata,
                        target_id,
                        task_id,
                    } => {
                        target_progress_bars[&target_id].tick();
                        let progress_bar =
                            multi_progress.add(ProgressBar::new(task_metadata.total_progress));
                        progress_bar.set_style(progress_style.clone());
                        progress_bar.set_prefix(get_prefix(&target_metadata, &task_metadata));
                        progress_bar.set_message("scheduled");
                        task_progress_bars.insert(task_id, progress_bar);
                    }
                    TargetSchedulerMessage::TaskStart {
                        target_id, task_id, ..
                    } => {
                        target_progress_bars[&target_id].tick();
                        let progress_bar = &task_progress_bars[&task_id];
                        progress_bar.set_message("start");
                    }
                    TargetSchedulerMessage::TaskProgress {
                        target_id,
                        task_id,
                        message,
                        position,
                        ..
                    } => {
                        target_progress_bars[&target_id].tick();
                        let progress_bar = &task_progress_bars[&task_id];
                        if let Some(message) = message {
                            progress_bar.set_message(message);
                        }
                        progress_bar.set_position(position);
                    }
                    TargetSchedulerMessage::TaskComplete {
                        target_id,
                        task_id,
                        target_metadata,
                        task_metadata,
                        result,
                    } => {
                        target_progress_bars[&target_id].tick();
                        let task_progress_bar = &task_progress_bars[&task_id];
                        let elapsed = task_progress_bar.elapsed();
                        task_progress_bar.finish_and_clear();
                        task_progress_bars.remove(&task_id);
                        match result {
                            Ok(()) => {
                                info!(
                                    "{} completes in {}.",
                                    get_prefix(&target_metadata, &task_metadata),
                                    HumanDuration(elapsed)
                                )
                            }
                            Err(e) => {
                                let prefix = get_prefix(&target_metadata, &task_metadata);
                                error!("{}: failed in {}.", prefix, HumanDuration(elapsed));
                                errors.push(e.context(prefix));
                            }
                        }
                    }
                    TargetSchedulerMessage::TargetScheduled {
                        target_metadata,
                        target_id,
                    } => {
                        let progress_bar = multi_progress.add(ProgressBar::new(1));
                        progress_bar.set_style(progress_style.clone());
                        progress_bar.set_prefix(target_metadata.name.clone());
                        progress_bar.set_message("scheduled");
                        target_progress_bars.insert(target_id, progress_bar);
                    }
                    TargetSchedulerMessage::TargetComplete {
                        target_id,
                        target_metadata,
                    } => {
                        let target_progress_bar = &target_progress_bars[&target_id];
                        let elapsed = target_progress_bar.elapsed();
                        target_progress_bar.finish_and_clear();
                        target_progress_bars.remove(&target_id);
                        info!(
                            "{} completes in {}.",
                            target_metadata.name,
                            HumanDuration(elapsed)
                        )
                    }
                }
            }
            errors
        })
        .context("spawn progress worker thread")?;

    ctrlc::set_handler({
        let cancellation_token_source = cancellation_token_source.clone();
        move || {
            cancellation_token_source.cancel();
        }
    })
    .context("set ctrlc handle")?;
    let res = target_graph.execute::<RayonTaskScheduler>(
        cli,
        move |message| {
            tx.send(message).unwrap_or_else(|e| {
                panic!("Failed to send the progress to the other thread: {:?}", e)
            })
        },
        cancellation_token_source,
    );
    let errors = match worker_thread.join() {
        Ok(errors) => errors,
        Err(e) => std::panic::resume_unwind(e),
    };
    for error in errors {
        error!("{:?}", error);
    }
    res
}
