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
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Error as AnyhowError, Result};
use thiserror::Error;
use xshell::{cmd, Cmd, Shell};

use crate::common::{
    run_cmd, CancellationToken, ProgressReport, TargetMetadata, TargetNode, Task, TaskContext,
    TaskError, TaskResult, TypedTask,
};

#[allow(clippy::enum_variant_names)]
#[derive(Error, Debug, Clone)]
pub(crate) enum DependencyError {
    #[error("rustup is missing. Follow https://rustup.rs/ to install rustup. {0}")]
    MissingRustup(Arc<AnyhowError>),
    #[error(
        "Nightly rustfmt is missing. Try `rustup toolchain install nightly --component rustfmt' \
         to install the nightly rustfmt. {0}"
    )]
    MissingRustfmt(Arc<AnyhowError>),
    #[error(
        "Nightly cargo is missing. Try `rustup toolchain install nightly --component cargo' to \
         install the nightly cargo. {0}"
    )]
    MissingNightlyCargo(Arc<AnyhowError>),
}

pub(crate) type DependencyResult<T> = Result<T, DependencyError>;

#[derive(Default)]
pub(crate) struct DependencyContext {
    pub rustfmt: Option<DependencyResult<PathBuf>>,
    pub cargo_fmt_cmd: Option<DependencyResult<CmdBuilder>>,
}

pub(crate) struct DependencyTarget;

impl TargetNode for DependencyTarget {
    fn metadata(&self) -> TargetMetadata {
        TargetMetadata {
            name: "depedency".to_owned(),
        }
    }

    fn create_tasks(
        &self,
        _: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<Box<dyn Task>>> {
        trait ResultExtension {
            fn map_failed<T: std::error::Error + Send + Sync + 'static>(
                self,
                f: impl FnOnce(Arc<AnyhowError>) -> T,
            ) -> Self;
        }
        impl<T> ResultExtension for TaskResult<T> {
            fn map_failed<U: std::error::Error + Send + Sync + 'static>(
                self,
                f: impl FnOnce(Arc<AnyhowError>) -> U,
            ) -> Self {
                self.map_err(|error| {
                    if let TaskError::Failed(failed_error) = error {
                        TaskError::Failed(AnyhowError::from(f(Arc::new(failed_error))))
                    } else {
                        error
                    }
                })
            }
        }
        struct RustfmtTask {
            cancellation_token: CancellationToken,
        }

        impl TypedTask for RustfmtTask {
            type OutputType = DependencyResult<PathBuf>;
            fn execute(
                &self,
                progress_report: &dyn ProgressReport,
            ) -> TaskResult<DependencyResult<PathBuf>> {
                let test = move || -> TaskResult<PathBuf> {
                    let sh = Shell::new().context("create shell")?;
                    run_cmd(
                        cmd!(sh, "rustup --version"),
                        progress_report,
                        self.cancellation_token.clone(),
                    )
                    .map_failed(DependencyError::MissingRustup)?;
                    let output = run_cmd(
                        cmd!(sh, "rustup which rustfmt --toolchain nightly"),
                        progress_report,
                        self.cancellation_token.clone(),
                    )
                    .map_failed(DependencyError::MissingRustfmt)?;
                    let rustfmt_path = String::from_utf8(output.stdout)
                        .context("The rustfmt path is not a valid UTF-8 string")?;
                    let rustfmt_path = rustfmt_path.trim_end().to_string();
                    Ok(rustfmt_path.into())
                };
                match test() {
                    Ok(rustfmt_path) => Ok(Ok(rustfmt_path)),
                    Err(e @ TaskError::Cancelled(_)) => Err(e),
                    Err(TaskError::Failed(e)) => match e.downcast::<DependencyError>() {
                        Ok(e) => Ok(Err(e)),
                        Err(e) => Ok(Err(DependencyError::MissingRustfmt(Arc::new(e)))),
                    },
                }
            }

            fn merge_output(
                &self,
                context: &mut TaskContext,
                output: Self::OutputType,
            ) -> TaskResult<()> {
                context.dependencies.rustfmt.replace(output);
                Ok(())
            }
        }

        struct CargoFmtTask {
            cancellation_token: CancellationToken,
        }

        impl TypedTask for CargoFmtTask {
            type OutputType = DependencyResult<CmdBuilder>;

            fn execute(
                &self,
                progress_report: &dyn ProgressReport,
            ) -> TaskResult<Self::OutputType> {
                let test = || -> TaskResult<CmdBuilder> {
                    let sh = Shell::new().context("create shell")?;
                    run_cmd(
                        cmd!(sh, "rustup --version"),
                        progress_report,
                        self.cancellation_token.clone(),
                    )
                    .map_failed(DependencyError::MissingRustup)?;
                    run_cmd(
                        cmd!(sh, "rustup run nightly cargo --version"),
                        progress_report,
                        self.cancellation_token.clone(),
                    )
                    .map_failed(DependencyError::MissingNightlyCargo)?;
                    run_cmd(
                        cmd!(sh, "rustup run nightly cargo fmt --version"),
                        progress_report,
                        self.cancellation_token.clone(),
                    )
                    .map_failed(DependencyError::MissingRustfmt)?;
                    Ok(Arc::new(|sh: &Shell| {
                        cmd!(sh, "rustup run nightly cargo fmt")
                    }))
                };
                match test() {
                    Ok(cargo_fmt_cmd) => Ok(Ok(cargo_fmt_cmd)),
                    Err(e @ TaskError::Cancelled(_)) => Err(e),
                    Err(TaskError::Failed(e)) => match e.downcast::<DependencyError>() {
                        Ok(e) => Ok(Err(e)),
                        Err(e) => Ok(Err(DependencyError::MissingRustfmt(Arc::new(e)))),
                    },
                }
            }

            fn merge_output(
                &self,
                context: &mut TaskContext,
                output: Self::OutputType,
            ) -> TaskResult<()> {
                context.dependencies.cargo_fmt_cmd.replace(output);
                Ok(())
            }
        }
        Ok(vec![
            Box::new(RustfmtTask {
                cancellation_token: cancellation_token.clone(),
            }),
            Box::new(CargoFmtTask {
                cancellation_token: cancellation_token.clone(),
            }),
        ])
    }
}

impl Default for DependencyTarget {
    fn default() -> Self {
        DependencyTarget
    }
}

pub(crate) type CmdBuilder = Arc<dyn for<'a> Fn(&'a Shell) -> Cmd<'a> + 'static + Send + Sync>;
