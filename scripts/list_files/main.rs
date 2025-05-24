// Copyright 2024 Google LLC
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

use itertools::Itertools;
use log::error;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use vulkan_layer_scripts::list_files::{parse_args, TaskBuilder};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::new()
        // TODO: !!! set the filter level based on github actions' debug env.
        .filter_level(log::LevelFilter::Trace)
        .init();

    let cli = match parse_args(std::env::args_os()) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };

    let paths = cli.get_paths().context("retrieve paths from CLI")?;
    let task_builder = TaskBuilder::new(cli.command.clone());

    let errors = std::thread::scope(move |s| {
        let errors: Arc<Mutex<Vec<anyhow::Error>>> = Arc::default();
        // Chunk every 1024 bytes to avoid the arguments too long on Windows.
        let path_chunks = paths
            .into_iter()
            .scan(0usize, |len, path| {
                *len += path.as_os_str().len();
                Some((*len, path))
            })
            .chunk_by(|(len, _)| len / 1024);
        for (_, chunk) in &path_chunks {
            if !errors.lock().unwrap().is_empty() {
                break;
            }
            let paths = chunk.map(|(_, path)| path);
            let task = task_builder.build_task(paths);
            s.spawn({
                let errors = errors.clone();
                move || {
                    if let Err(e) = task.run(&mut std::io::stdout(), &mut std::io::stderr()) {
                        errors.lock().unwrap().push(e);
                    }
                }
            });
        }
        errors.clone()
    });

    let errors = errors.lock().unwrap();
    for error in errors.iter() {
        error!("Failed to complete task: {:?}", error);
    }
    if !errors.is_empty() {
        bail!("Failed to complete all tasks");
    }
    Ok(())
}
