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
    collections::BTreeSet,
    fmt::Display,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use ignore::{types::TypesBuilder, WalkBuilder};
use log::{error, trace};
use xshell::cmd;

use crate::common::{
    CancellationToken, CmdsTask, ProgressReport, Target, TargetMetadata, TargetNode, Task,
    TaskContext, TypedTask,
};

pub(crate) struct FormatFilesContext {
    markdown_files: Vec<PathBuf>,
}

pub(crate) struct FormatConfig {
    pub wrap: u32,
    pub check: bool,
    pub markdown_end_of_line: MarkdownFormatEndOfLine,
}

impl Default for FormatConfig {
    fn default() -> Self {
        #[cfg(unix)]
        let markdown_end_of_line = MarkdownFormatEndOfLine::Lf;
        #[cfg(windows)]
        let markdown_end_of_line = MarkdownFormatEndOfLine::Crlf;
        Self {
            wrap: 100,
            check: false,
            markdown_end_of_line,
        }
    }
}

#[derive(Default)]
pub(crate) struct FormatTarget;

impl TargetNode for FormatTarget {
    fn metadata(&self) -> TargetMetadata {
        TargetMetadata {
            name: "fmt".to_owned(),
        }
    }

    fn dependencies(&self) -> BTreeSet<Target> {
        BTreeSet::from([Target::FormatMarkdown])
    }

    fn create_tasks(&self, _: Arc<Mutex<TaskContext>>, _: CancellationToken) -> Vec<Box<dyn Task>> {
        vec![]
    }
}

#[derive(Clone)]
pub(crate) enum MarkdownFormatEndOfLine {
    #[cfg(unix)]
    Lf,
    #[cfg(windows)]
    Crlf,
}

impl Display for MarkdownFormatEndOfLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let eol = match self {
            #[cfg(unix)]
            MarkdownFormatEndOfLine::Lf => "lf",
            #[cfg(windows)]
            MarkdownFormatEndOfLine::Crlf => "crlf",
        };
        write!(f, "{}", eol)
    }
}

#[derive(Default)]
pub(crate) struct FormatMarkdownTarget;

impl TargetNode for FormatMarkdownTarget {
    fn metadata(&self) -> TargetMetadata {
        TargetMetadata {
            name: "fmt_md".to_owned(),
        }
    }

    fn dependencies(&self) -> BTreeSet<Target> {
        BTreeSet::from([Target::FormatDiscoverFiles])
    }

    fn create_tasks(
        &self,
        context: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Vec<Box<dyn Task>> {
        let format_config = Arc::clone(&context.lock().unwrap().format_config);
        let format_files = context.lock().unwrap().format_files.clone();
        let format_files = format_files.expect(
            "Format file contexts are missing. Please check if all dependencies are properly \
             specified.",
        );
        format_files
            .markdown_files
            .iter()
            .map(|file| -> Box<dyn Task> {
                let task_name = match file.file_name() {
                    Some(file_name) => file_name.to_string_lossy().to_string(),
                    None => file.display().to_string(),
                };
                let task = CmdsTask::new(
                    Some(task_name),
                    {
                        let format_config = Arc::clone(&format_config);
                        let file = file.to_owned();
                        move |sh| {
                            let wrap = format_config.wrap.to_string();
                            let eol = format_config.markdown_end_of_line.to_string();
                            let mut cmd =
                                cmd!(sh, "pipenv run mdformat --wrap {wrap} --end-of-line {eol}");
                            if format_config.check {
                                cmd = cmd.arg("--check");
                            }
                            vec![cmd.arg(&file)]
                        }
                    },
                    cancellation_token.clone(),
                );
                Box::new(task)
            })
            .collect()
    }
}

#[derive(Default)]
pub(crate) struct FormatDiscoverFilesTarget;

impl TargetNode for FormatDiscoverFilesTarget {
    fn metadata(&self) -> TargetMetadata {
        TargetMetadata {
            name: "discover_fmt".to_owned(),
        }
    }

    fn dependencies(&self) -> BTreeSet<Target> {
        BTreeSet::new()
    }

    fn create_tasks(
        &self,
        _: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Vec<Box<dyn Task>> {
        struct FormatDiscoverFilesTask {
            cancellation_token: CancellationToken,
        }

        impl TypedTask for FormatDiscoverFilesTask {
            type OutputType = FormatFilesContext;

            fn merge_output(
                &self,
                context: &mut TaskContext,
                output: Self::OutputType,
            ) -> Result<()> {
                context.format_files = Some(Arc::new(output));
                Ok(())
            }

            fn execute(
                &self,
                progress_report: Box<dyn ProgressReport>,
            ) -> Result<Self::OutputType> {
                self.cancellation_token.check_cancelled()?;
                let md_type = TypesBuilder::new()
                    .add_defaults()
                    .select("md")
                    .build()
                    .context("build the markdown type filter")?;
                self.cancellation_token.check_cancelled()?;
                let files = WalkBuilder::new("./").types(md_type).build();
                let mut markdown_files = vec![];
                for res in files {
                    self.cancellation_token.check_cancelled()?;
                    let entry = match res {
                        Err(e) => {
                            error!("{}", e);
                            continue;
                        }
                        Ok(entry) => entry,
                    };
                    let is_file = entry
                        .file_type()
                        .map(|file_type| file_type.is_file())
                        .unwrap_or(false);
                    if !is_file {
                        trace!("Skip directory {}", entry.path().display());
                        continue;
                    }
                    progress_report.set_message(format!("discovered: {}", entry.path().display()));
                    markdown_files.push(entry.path().to_owned())
                }
                Ok(FormatFilesContext { markdown_files })
            }
        }

        vec![Box::new(FormatDiscoverFilesTask { cancellation_token })]
    }
}
