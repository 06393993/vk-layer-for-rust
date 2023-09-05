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

use anyhow::{Context, Error, Result};
use once_cell::sync::Lazy;
use rayon::prelude::{IntoParallelRefIterator, ParallelIterator};
use std::{
    collections::BTreeSet,
    env,
    fs::File,
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::SystemTime,
};
use xshell::{cmd, Shell, TempDir};

use bindgen::EnumVariation;

use crate::common::{
    CancellationToken, CmdsTask, ProgressReport, SimpleTypedTask, Target, TargetMetadata,
    TargetNode, Task, TaskContext, TaskContextExt, TaskMetadata, TaskResult,
};

// May not be precise, but should be enough for generating copyright comments.
fn current_year() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    const SECONDS_PER_YEAR: u64 = 60 * 60 * 24 * 365;
    since_epoch.as_secs() / SECONDS_PER_YEAR + 1970
}

fn diff_files(
    left_path: &Path,
    right_path: &Path,
    progress_report: &dyn ProgressReport,
    cancellation_token: CancellationToken,
) -> Result<()> {
    let mut contents = [left_path, right_path]
        .par_iter()
        .map(|file_path| -> Result<String> {
            let mut file = File::open(file_path).context("open file")?;
            cancellation_token.check_cancelled()?;
            progress_report.set_message(format!("Reading contents from {}", file_path.display()));
            let mut buffer = String::new();
            file.read_to_string(&mut buffer)
                .with_context(|| format!("read contents from {}", file_path.display()))?;
            Ok(buffer)
        })
        .collect::<Vec<_>>();

    let right = contents.pop().unwrap()?;
    let left = contents.pop().unwrap()?;
    let diff_lines = diff::lines(&left, &right);
    let line_width: usize = diff_lines
        .len()
        .checked_ilog10()
        .unwrap_or(0)
        .try_into()
        .unwrap();
    let is_same = diff_lines
        .iter()
        .all(|diff| matches!(diff, diff::Result::Both(..)));
    let mut error_message = BufWriter::new(Vec::<u8>::new());
    let mut left_line_number = 0;
    let mut right_line_number = 0;
    writeln!(
        error_message,
        "{} and {} don't match",
        left_path.display(),
        right_path.display()
    )
    .context("write general error message")?;
    writeln!(error_message).context("write blank line to the error message")?;
    for diff in diff_lines {
        match diff {
            diff::Result::Left(left) => {
                left_line_number += 1;
                writeln!(
                    error_message,
                    "{:>line_width$} {:>line_width$} - {}",
                    left_line_number, "", left
                )
                .context("write a missing line")?;
            }
            diff::Result::Right(right) => {
                right_line_number += 1;
                writeln!(
                    error_message,
                    "{:>line_width$} {:>line_width$} + {}",
                    "", right_line_number, right
                )
                .context("write an extra line")?;
            }
            diff::Result::Both(..) => {
                left_line_number += 1;
                right_line_number += 1;
            }
        }
    }
    if !is_same {
        let error_message = error_message
            .into_inner()
            .context("unwrap the buffered writer for the error message")?;
        let error_message = String::from_utf8_lossy(&error_message).to_string();
        return Err(Error::msg(error_message));
    }
    Ok(())
}

struct BindgenTask {
    rustfmt_path: PathBuf,
    out_path: PathBuf,
    preamble: String,
    bindgen_builder_builder: Box<dyn Fn(bindgen::Builder) -> bindgen::Builder + 'static + Send>,
    cancellation_token: CancellationToken,
}

impl SimpleTypedTask for BindgenTask {
    fn metadata(&self) -> TaskMetadata {
        TaskMetadata {
            name: Some(match self.out_path.file_name() {
                Some(out_path) => out_path.to_string_lossy().to_string(),
                None => self.out_path.display().to_string(),
            }),
            total_progress: 1,
        }
    }

    fn execute(&self, progress_report: &dyn ProgressReport) -> TaskResult<()> {
        self.cancellation_token.check_cancelled()?;
        let mut out_file = File::create(&self.out_path).with_context(|| {
            format!(
                "creating output file for bindgen: {}",
                self.out_path.display()
            )
        })?;
        self.cancellation_token.check_cancelled()?;
        progress_report.set_message("writing preambles".to_string());
        out_file
            .write_all(self.preamble.as_bytes())
            .with_context(|| format!("writing preamble to {}", self.out_path.display()))?;
        self.cancellation_token.check_cancelled()?;
        let bindgen_builder = bindgen::Builder::default().with_rustfmt(&self.rustfmt_path);
        let bindgen_builder = (self.bindgen_builder_builder)(bindgen_builder);
        progress_report.set_message("generating content".to_string());
        let bindings = bindgen_builder
            .generate()
            .with_context(|| format!("generating bindings for {}", self.out_path.display()))?;
        self.cancellation_token.check_cancelled()?;
        progress_report.set_message("writing generated contents".to_string());
        bindings.write(Box::new(out_file)).with_context(|| {
            format!("writing generated contents to {}", self.out_path.display())
        })?;
        progress_report.finish();
        Ok(())
    }
}

fn guess_python_command() -> Option<&'static str> {
    ["python3", "python", "py"].into_iter().find(|cmd| {
        let output = match Command::new(cmd).arg("--version").output() {
            Ok(output) => output,
            Err(_) => return false,
        };
        output.status.success()
    })
}

pub(crate) struct CodegenTarget {
    vulkan_headers_dir: PathBuf,
    vk_layer_general_out: PathBuf,
    vk_layer_platform_specific_out: PathBuf,
    vulkan_layer_genvk_path: PathBuf,
    genvk_targets: Vec<&'static str>,
    vk_xml_path: PathBuf,
    vulkan_layer_generated_dir: PathBuf,
    rustfmt_config_file_path: PathBuf,
}

impl CodegenTarget {
    fn create_vk_layer_binding_tasks(
        &self,
        context: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<Box<dyn Task>>> {
        let rustfmt_path = context.rustfmt_path().context("check rustfmt")?;

        let mut vulkan_headers_include_dir = self.vulkan_headers_dir.clone();
        vulkan_headers_include_dir.push("include");
        let mut vk_layer_h_path = vulkan_headers_include_dir.clone();
        vk_layer_h_path.push("vulkan");
        vk_layer_h_path.push("vk_layer.h");

        let license_comments = format!(
            "// Copyright {} Google LLC\n\
             //\n\
             // Licensed under the Apache License, Version 2.0 (the \"License\");\n\
             // you may not use this file except in compliance with the License.\n\
             // You may obtain a copy of the License at\n\
             //\n\
             //     http://www.apache.org/licenses/LICENSE-2.0\n\
             //\n\
             // Unless required by applicable law or agreed to in writing, software\n\
             // distributed under the License is distributed on an \"AS IS\" BASIS,\n\
             // WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.\n\
             // See the License for the specific language governing permissions and\n\
             // limitations under the License.\n\n",
            current_year()
        );
        let set_common_bindgen_configs = {
            let rustfmt_config_file_path = self.rustfmt_config_file_path.clone();
            move |bindgen_builder: bindgen::Builder| -> bindgen::Builder {
                bindgen_builder
                    .rustfmt_configuration_file(Some(rustfmt_config_file_path.clone()))
                    .header(vk_layer_h_path.as_os_str().to_str().unwrap())
                    .clang_args(&[
                        "-I",
                        vulkan_headers_include_dir.as_os_str().to_str().unwrap(),
                    ])
                    .default_enum_style(EnumVariation::NewType {
                        is_bitfield: false,
                        is_global: false,
                    })
                    .derive_default(true)
                    // We expect ash to cover most type definitions for us.
                    .allowlist_recursively(false)
            }
        };
        let mut preamble = license_comments.clone();
        preamble.push_str(
            "#![allow(missing_docs)]
use super::*;
use ash::vk::*;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;
",
        );
        let temp_dir = Arc::new(Lazy::new(|| {
            let sh = Shell::new().unwrap_or_else(|e| {
                panic!("Failed to create shell for creating temp dir: {:?}", e)
            });
            sh.create_temp_dir()
                .unwrap_or_else(|e| panic!("Failed to create temporary directory: {:?}", e))
        }));
        let check = context
            .lock()
            .unwrap()
            .codegen_cli
            .as_ref()
            .map(|cli| cli.check)
            .unwrap_or(false);
        let general_out_path = if check {
            let mut out_path = temp_dir.path().to_owned();
            out_path.push("vk_layer_general.rs");
            out_path
        } else {
            self.vk_layer_general_out.clone()
        };
        let general_binding_task = BindgenTask {
            rustfmt_path: rustfmt_path.clone(),
            out_path: general_out_path.clone(),
            preamble,
            bindgen_builder_builder: Box::new({
                let set_common_bindgen_configs = set_common_bindgen_configs.clone();
                move |bindgen_builder: bindgen::Builder| -> bindgen::Builder {
                    set_common_bindgen_configs(bindgen_builder)
                        .allowlist_file("vk_layer.h")
                        .allowlist_type("VkNegotiateLayerInterface")
                        .allowlist_type("VkLayerDeviceCreateInfo")
                        .allowlist_type("PFN_GetPhysicalDeviceProcAddr")
                        .allowlist_type("VkLayerInstanceLink_?")
                        .allowlist_type("PFN_vkSetInstanceLoaderData")
                        .allowlist_type("PFN_vkLayerCreateDevice")
                        .allowlist_type("PFN_vkLayerDestroyDevice")
                        .allowlist_type("VkLayerDeviceLink_?")
                        .allowlist_type("PFN_vkSetDeviceLoaderData")
                }
            }),
            cancellation_token: cancellation_token.clone(),
        };
        let general_binding_task = general_binding_task.and_then({
            let cancellation_token = cancellation_token.clone();
            let expected_path = general_out_path;
            let actual_path = self.vk_layer_general_out.clone();
            let temp_dir = temp_dir.clone();
            move |progress_report| {
                cancellation_token.check_cancelled()?;
                if expected_path == actual_path {
                    assert!(!check);
                    return Ok(());
                }
                diff_files(
                    &actual_path,
                    &expected_path,
                    progress_report,
                    cancellation_token.clone(),
                )
                .with_context(|| format!("check {}", actual_path.display()))?;
                // We must keep temp_dir alive so that the directory is still alive.
                drop(temp_dir.clone());
                Ok(())
            }
        });
        let platform_specific_out_path = if check {
            let mut out_path = temp_dir.path().to_owned();
            out_path.push("platform.rs");
            out_path
        } else {
            self.vk_layer_platform_specific_out.clone()
        };
        let platform_specific_binding_task = BindgenTask {
            rustfmt_path,
            out_path: platform_specific_out_path.clone(),
            preamble: license_comments,
            bindgen_builder_builder: Box::new({
                let set_common_bindgen_configs = set_common_bindgen_configs;
                move |builder: bindgen::Builder| -> bindgen::Builder {
                    set_common_bindgen_configs(builder)
                        .allowlist_type("VkLayerFunction_?")
                        .allowlist_type("VkNegotiateLayerStructType")
                }
            }),
            cancellation_token: cancellation_token.clone(),
        };
        let platform_specific_binding_task = platform_specific_binding_task.and_then({
            let expected_path = platform_specific_out_path;
            let actual_path = self.vk_layer_platform_specific_out.clone();
            move |progress_report| {
                cancellation_token.check_cancelled()?;
                diff_files(
                    &actual_path,
                    &expected_path,
                    progress_report,
                    cancellation_token.clone(),
                )
                .with_context(|| format!("check {}", actual_path.display()))?;
                // We must keep temp_dir alive so that the directory is still alive.
                drop(temp_dir.clone());
                Ok(())
            }
        });
        Ok(vec![
            Box::new(general_binding_task),
            Box::new(platform_specific_binding_task),
        ])
    }

    fn create_vulkan_layer_genvk_tasks(
        &self,
        context: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<Box<dyn Task>>> {
        let check = context
            .lock()
            .unwrap()
            .codegen_cli
            .as_ref()
            .map(|cli| cli.check)
            .unwrap_or(false);
        let temp_dir = Arc::new(Mutex::<Option<TempDir>>::new(None));
        let mut tasks: Vec<Box<dyn Task>> = vec![];
        // TODO: move python detection to a separate target
        let python_command = guess_python_command().expect("Failed to find installed python.");
        for target in &self.genvk_targets {
            let task_name = target.to_string();
            let target = PathBuf::from(target);
            let task = CmdsTask::new(
                Some(task_name),
                {
                    let vulkan_layer_genvk_path = self.vulkan_layer_genvk_path.clone();
                    let registry = self.vk_xml_path.clone();
                    let temp_dir = Arc::clone(&temp_dir);
                    let vulkan_layer_generated_dir = self.vulkan_layer_generated_dir.clone();
                    let target = target.clone();
                    let rustfmt_config_file_path = self.rustfmt_config_file_path.clone();
                    let rustfmt_path = context.rustfmt_path().context("check rustfmt")?;
                    move |shell| {
                        let out_dir = if check {
                            let mut temp_dir = temp_dir.lock().unwrap();
                            match temp_dir.as_ref() {
                                Some(temp_dir) => temp_dir.path().to_owned(),
                                None => {
                                    let dir = shell.create_temp_dir().unwrap_or_else(|e| {
                                        panic!("Failed to create temporary out directory: {:?}", e)
                                    });
                                    let path = dir.path().to_owned();
                                    *temp_dir = Some(dir);
                                    path
                                }
                            }
                        } else {
                            vulkan_layer_generated_dir.clone()
                        };
                        let mut output_file_path = out_dir.clone();
                        output_file_path.push(target.clone());
                        vec![
                            cmd!(shell, "{python_command} {vulkan_layer_genvk_path} {target}")
                                .arg("-registry")
                                .arg(&registry)
                                .arg("-o")
                                .arg(&out_dir),
                            cmd!(shell, "{rustfmt_path}")
                                .arg("--unstable-features")
                                .arg("--skip-children")
                                .arg("--edition")
                                .arg("2021")
                                .arg("--config-path")
                                .arg(&rustfmt_config_file_path)
                                .arg(&output_file_path),
                        ]
                    }
                },
                cancellation_token.clone(),
            );
            let task = task.and_then({
                let temp_dir = Arc::clone(&temp_dir);
                let cancellation_token = cancellation_token.clone();
                let actual_dir = self.vulkan_layer_generated_dir.clone();
                move |progress_report| {
                    cancellation_token.check_cancelled()?;
                    let expected_dir = {
                        let temp_dir = temp_dir.lock().unwrap();
                        match temp_dir.as_ref() {
                            Some(temp_dir) => {
                                assert!(check, "Temporary directory not created for checking.");
                                temp_dir.path().to_owned()
                            }
                            None => return Ok(()),
                        }
                    };
                    let mut actual_file_path = actual_dir.clone();
                    actual_file_path.push(target.clone());
                    let mut expected_file_path = expected_dir;
                    expected_file_path.push(target.clone());
                    diff_files(
                        &actual_file_path,
                        &expected_file_path,
                        progress_report,
                        cancellation_token.clone(),
                    )
                    .with_context(|| format!("check {}", target.display()))?;
                    Ok(())
                }
            });
            tasks.push(Box::new(task));
        }
        Ok(tasks)
    }
}

impl TargetNode for CodegenTarget {
    fn metadata(&self) -> TargetMetadata {
        TargetMetadata {
            name: "codegen".to_string(),
        }
    }

    fn dependencies(&self) -> BTreeSet<Target> {
        BTreeSet::from([Target::Dependency])
    }

    fn create_tasks(
        &self,
        context: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<Box<dyn Task>>> {
        let mut tasks = vec![];
        tasks.extend(
            self.create_vk_layer_binding_tasks(Arc::clone(&context), cancellation_token.clone())
                .context("create vk_layer.h bindgen tasks")?,
        );
        tasks.extend(
            self.create_vulkan_layer_genvk_tasks(context, cancellation_token)
                .context("generate from the vulkan_layer_genvk.py script")?,
        );
        Ok(tasks)
    }
}

impl Default for CodegenTarget {
    fn default() -> Self {
        let cargo_manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        assert!(cargo_manifest_dir.is_absolute());
        let mut project_root_dir = cargo_manifest_dir;
        assert!(project_root_dir.pop());
        let mut rustfmt_config_file_path = project_root_dir.clone();
        rustfmt_config_file_path.push("rustfmt.toml");
        assert!(
            rustfmt_config_file_path.exists(),
            "The rustfmt config file {} doesn't exist.",
            rustfmt_config_file_path.display()
        );

        let mut vulkan_headers_dir = project_root_dir.clone();
        vulkan_headers_dir.push("third_party");
        vulkan_headers_dir.push("Vulkan-Headers");

        let mut vulkan_layer_dir = project_root_dir.clone();
        vulkan_layer_dir.push("vulkan-layer");
        let mut vk_layer_general_out = vulkan_layer_dir.clone();
        vk_layer_general_out.push("src");
        vk_layer_general_out.push("bindings");
        vk_layer_general_out.push("vk_layer");
        let mut vk_layer_platform_specific_out = vk_layer_general_out.clone();
        vk_layer_general_out.push("generated.rs");
        vk_layer_platform_specific_out.push("generated");
        #[cfg(windows)]
        vk_layer_platform_specific_out.push("windows.rs");
        #[cfg(unix)]
        platform_specific_out.push("unix.rs");

        let mut vulkan_layer_genvk_path = project_root_dir.clone();
        vulkan_layer_genvk_path.push("scripts");
        vulkan_layer_genvk_path.push("vulkan_layer_genvk.py");

        let mut vk_xml_path = vulkan_headers_dir.clone();
        vk_xml_path.push("registry");
        vk_xml_path.push("vk.xml");

        let mut vulkan_layer_generated_dir = project_root_dir.clone();
        vulkan_layer_generated_dir.push("vulkan-layer");
        vulkan_layer_generated_dir.push("src");

        Self {
            vulkan_headers_dir,
            vk_layer_general_out,
            vk_layer_platform_specific_out,
            vulkan_layer_genvk_path,
            genvk_targets: vec![
                "layer_trait/generated.rs",
                "global_simple_intercept/generated.rs",
            ],
            vk_xml_path,
            vulkan_layer_generated_dir,
            rustfmt_config_file_path,
        }
    }
}
