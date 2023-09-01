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

use anyhow::{Context, Result};
use std::{
    collections::BTreeSet,
    env,
    fs::File,
    io::Write,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::SystemTime,
};
use xshell::cmd;

use bindgen::EnumVariation;

use crate::common::{
    CancellationToken, CmdsTask, ProgressReport, SimpleTypedTask, Target, TargetMetadata,
    TargetNode, Task, TaskContext, TaskMetadata,
};

// May not be precise, but should be enough for generating copyright comments.
fn current_year() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    const SECONDS_PER_YEAR: u64 = 60 * 60 * 24 * 365;
    since_epoch.as_secs() / SECONDS_PER_YEAR + 1970
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

    fn execute(&self, progress_report: Box<dyn ProgressReport>) -> Result<()> {
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
}

impl CodegenTarget {
    fn create_vk_layer_binding_tasks(
        &self,
        _: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Vec<Box<dyn Task>> {
        // TODO: move rustfmt detection to a separate software detection task
        let rustfmt_path = {
            let output = Command::new("rustup")
                .args(["which", "rustfmt", "--toolchain", "nightly"])
                .output()
                .expect("Could not spawn `rustup` command");
            assert!(
                output.status.success(),
                "Unsuccessful status code when running `rustup`: {:?}",
                output
            );
            String::from_utf8(output.stdout).expect("The `rustfmt` path is not valid `utf-8`")
        };
        let rustfmt_path = rustfmt_path.trim_end();
        let rustfmt_path = PathBuf::from(rustfmt_path);

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
            move |bindgen_builder: bindgen::Builder| -> bindgen::Builder {
                bindgen_builder
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
        let general_binding_task = BindgenTask {
            rustfmt_path: rustfmt_path.clone(),
            out_path: self.vk_layer_general_out.clone(),
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
        let platform_specific_binding_task = BindgenTask {
            rustfmt_path,
            out_path: self.vk_layer_platform_specific_out.clone(),
            preamble: license_comments,
            bindgen_builder_builder: Box::new({
                let set_common_bindgen_configs = set_common_bindgen_configs.clone();
                move |builder: bindgen::Builder| -> bindgen::Builder {
                    set_common_bindgen_configs(builder)
                        .allowlist_type("VkLayerFunction_?")
                        .allowlist_type("VkNegotiateLayerStructType")
                }
            }),
            cancellation_token,
        };
        vec![
            Box::new(general_binding_task),
            Box::new(platform_specific_binding_task),
        ]
    }

    fn create_vulkan_layer_genvk_tasks(
        &self,
        _: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Vec<Box<dyn Task>> {
        let mut tasks: Vec<Box<dyn Task>> = vec![];
        // TODO: move python detection to a separate target
        let python_command = guess_python_command().expect("Failed to find installed python.");
        for target in &self.genvk_targets {
            let task_name = target.to_string();
            let target = PathBuf::from(target);
            let mut output_file_path = self.vulkan_layer_generated_dir.clone();
            output_file_path.push(target.clone());
            let task = CmdsTask::new(
                Some(task_name),
                {
                    let python_command = python_command.clone();
                    let vulkan_layer_genvk_path = self.vulkan_layer_genvk_path.clone();
                    let registry = self.vk_xml_path.clone();
                    let out_dir = self.vulkan_layer_generated_dir.clone();
                    move |shell| {
                        vec![
                            cmd!(shell, "{python_command} {vulkan_layer_genvk_path} {target}")
                                .arg("-registry")
                                .arg(&registry)
                                .arg("-o")
                                .arg(&out_dir),
                            // TODO: check if rustup and rustfmt nightly exist on the system
                            cmd!(shell, "rustup run nightly rustfmt")
                                .arg("--unstable-features")
                                .arg("--skip-children")
                                .arg("--edition")
                                .arg("2021")
                                .arg(&output_file_path),
                        ]
                    }
                },
                cancellation_token.clone(),
            );
            tasks.push(Box::new(task));
        }
        tasks
    }
}

impl TargetNode for CodegenTarget {
    fn metadata(&self) -> TargetMetadata {
        TargetMetadata {
            name: "codegen".to_string(),
        }
    }

    fn dependencies(&self) -> BTreeSet<Target> {
        BTreeSet::new()
    }

    fn create_tasks(
        &self,
        context: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Vec<Box<dyn Task>> {
        let mut tasks = vec![];
        tasks.extend(
            self.create_vk_layer_binding_tasks(Arc::clone(&context), cancellation_token.clone()),
        );
        tasks.extend(self.create_vulkan_layer_genvk_tasks(context, cancellation_token));
        tasks
    }
}

impl Default for CodegenTarget {
    fn default() -> Self {
        let cargo_manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        assert!(cargo_manifest_dir.is_absolute());
        let mut project_root_dir = cargo_manifest_dir;
        assert!(project_root_dir.pop());

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
        }
    }
}
