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
    fs::File,
    io::BufReader,
    sync::{Arc, Mutex},
};

use anyhow::{ensure, Context, Result};
use log::error;
use serde::{Deserialize, Serialize};

use crate::{
    common::{
        CancellationToken, SimpleTypedTask, Target, TargetMetadata, TargetNode, Task, TaskContext,
        TaskMetadata,
    },
    CiCli,
};

#[derive(Deserialize)]
struct CoverageStats {
    percent: f64,
}

#[derive(Deserialize)]
struct CoverageReportDataTotals {
    lines: CoverageStats,
}

#[derive(Deserialize)]
struct CoverageReportData {
    totals: CoverageReportDataTotals,
}

#[derive(Deserialize)]
struct CoverageReport {
    data: Vec<CoverageReportData>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShieldBadge {
    schema_version: u32,
    label: String,
    message: String,
    color: String,
}

fn create_shield_badge(label: String, coverage_stats: &CoverageStats) -> ShieldBadge {
    let percent = coverage_stats.percent;
    let color = if (0.0..75.0).contains(&percent) {
        "#e05d44"
    } else if (75.0..90.0).contains(&percent) {
        "#dfb317"
    } else if (90.0..95.0).contains(&percent) {
        "#a3c51c"
    } else if (95.0..100.0).contains(&percent) {
        "#4c1"
    } else {
        "#9f9f9f"
    };
    let message = if (0.0..=100.0).contains(&percent) {
        format!("{:.1}%", percent)
    } else {
        error!("Invalid percentage: {}", percent);
        "unknown".to_owned()
    };
    ShieldBadge {
        schema_version: 1,
        label,
        message,
        color: color.to_owned(),
    }
}

struct CoverageBadgeTask {
    cancellation_token: CancellationToken,
    ci_cli: CiCli,
}

impl SimpleTypedTask for CoverageBadgeTask {
    fn execute(&self, progress_report: Box<dyn crate::common::ProgressReport>) -> Result<()> {
        let cli = &self.ci_cli;
        self.cancellation_token.check_cancelled()?;
        ensure!(
            cli.coverage_report.exists(),
            "{} doesn't exist.",
            cli.coverage_report.display()
        );
        self.cancellation_token.check_cancelled()?;
        let file = File::open(&cli.coverage_report).with_context(|| {
            format!("open coverage report at {}", cli.coverage_report.display())
        })?;
        self.cancellation_token.check_cancelled()?;
        let reader = BufReader::new(file);
        let report: CoverageReport = serde_json::from_reader(reader)
            .with_context(|| format!("parse the report at {}", cli.coverage_report.display()))?;
        self.cancellation_token.check_cancelled()?;
        ensure!(
            !report.data.is_empty(),
            "Unexpected data field length: {}",
            report.data.len()
        );
        let shield_badge = create_shield_badge(cli.label.clone(), &report.data[0].totals.lines);
        self.cancellation_token.check_cancelled()?;
        let mut out_file = File::create(&cli.output_path).with_context(|| {
            format!(
                "create output file for shield badge at {}",
                cli.output_path.display()
            )
        })?;
        self.cancellation_token.check_cancelled()?;
        serde_json::to_writer(&mut out_file, &shield_badge)
            .with_context(|| format!("write the sheild badge to {}", cli.output_path.display()))?;
        progress_report.finish();
        Ok(())
    }

    fn metadata(&self) -> TaskMetadata {
        TaskMetadata {
            name: Some("cov_badge".to_owned()),
            total_progress: 1,
        }
    }
}

#[derive(Default)]
pub(crate) struct CiTarget;

impl TargetNode for CiTarget {
    fn metadata(&self) -> TargetMetadata {
        TargetMetadata {
            name: "ci".to_owned(),
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
        let ci_cli = context
            .lock()
            .unwrap()
            .ci_cli
            .take()
            .expect("missing CI CLI");
        vec![Box::new(CoverageBadgeTask {
            cancellation_token,
            ci_cli,
        })]
    }
}
