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
    cell::RefCell,
    collections::{btree_map, BTreeMap, BTreeSet},
    io::{BufWriter, Write},
    process::Stdio,
    rc::Rc,
    sync::{
        atomic::AtomicBool,
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

use anyhow::{anyhow, Context, Error, Result};
use fixedbitset::FixedBitSet;
use log::{debug, error, warn};
use petgraph::{
    graph::{Graph, NodeIndex},
    visit::{IntoNeighbors, IntoNodeIdentifiers, Reversed, VisitMap, Visitable},
    Direction::Incoming,
};
use wait_timeout::ChildExt;
use xshell::{Cmd, Shell};

use crate::{
    ci::CiTarget,
    codegen::CodegenTarget,
    fmt::{
        FormatConfig, FormatDiscoverFilesTarget, FormatFilesContext, FormatMarkdownTarget,
        FormatTarget,
    },
    CiCli, Commands, FmtFileType,
};

pub(crate) struct CancellationTokenSource(Arc<AtomicBool>);

impl CancellationTokenSource {
    pub fn token(&self) -> CancellationToken {
        CancellationToken(Arc::clone(&self.0))
    }

    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for CancellationTokenSource {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}

#[derive(Clone)]
pub(crate) struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(anyhow!("Canceled"))
        } else {
            Ok(())
        }
    }
}

pub(crate) struct TaskContext {
    pub format_config: Arc<FormatConfig>,
    pub format_files: Option<Arc<FormatFilesContext>>,
    pub ci_cli: Option<CiCli>,
}

impl From<super::Cli> for TaskContext {
    fn from(cli: super::Cli) -> Self {
        let mut format_config = FormatConfig::default();
        let mut ci_cli = None;
        match cli.command {
            Commands::Fmt(fmt_cli) => format_config.check = fmt_cli.check,
            Commands::Ci(input) => ci_cli = Some(input),
            Commands::Codegen => {}
        }
        Self {
            format_files: None,
            format_config: Arc::new(format_config),
            ci_cli,
        }
    }
}

impl super::Cli {
    pub(crate) fn create_targets(&self) -> BTreeSet<Target> {
        match &self.command {
            super::Commands::Ci(_) => BTreeSet::from([Target::Ci]),
            super::Commands::Codegen => BTreeSet::from([Target::Codegen]),
            super::Commands::Fmt(fmt_cli) => {
                let file_types = fmt_cli.file_type.iter().cloned().collect::<BTreeSet<_>>();
                if file_types.is_empty() {
                    return BTreeSet::from([Target::Format]);
                }
                file_types
                    .into_iter()
                    .map(|file_type| match file_type {
                        FmtFileType::Markdown => Target::FormatMarkdown,
                    })
                    .collect::<BTreeSet<_>>()
            }
        }
    }
}

pub(crate) trait ProgressReport {
    fn set_message(&self, message: String);
    fn increase(&self, delta: u64);
    fn finish(&self);
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Target {
    FormatDiscoverFiles,
    FormatMarkdown,
    Format,
    Codegen,
    Ci,
}

#[derive(Clone)]
pub(crate) struct TargetMetadata {
    pub name: String,
}

pub(crate) trait TargetNode {
    fn metadata(&self) -> &TargetMetadata;
    fn dependencies(&self) -> BTreeSet<Target>;
    fn create_tasks(
        &self,
        context: Arc<Mutex<TaskContext>>,
        cancellation_token: CancellationToken,
    ) -> Vec<Box<dyn Task>>;
}

impl From<Target> for Box<dyn TargetNode> {
    fn from(value: Target) -> Self {
        match value {
            Target::Format => Box::<FormatTarget>::default(),
            Target::FormatMarkdown => Box::<FormatMarkdownTarget>::default(),
            Target::FormatDiscoverFiles => Box::<FormatDiscoverFilesTarget>::default(),
            Target::Codegen => Box::<CodegenTarget>::default(),
            Target::Ci => Box::<CiTarget>::default(),
        }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone)]
pub(crate) struct TaskId(u32);

impl From<u32> for TaskId {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

#[derive(Clone)]
pub(crate) struct TaskMetadata {
    pub name: Option<String>,
    pub total_progress: u64,
}

impl Default for TaskMetadata {
    fn default() -> Self {
        Self {
            name: None,
            total_progress: 1,
        }
    }
}

pub(crate) trait Task: Send {
    fn metadata(&self) -> &TaskMetadata;
    fn execute(
        &self,
        context: Arc<Mutex<TaskContext>>,
        progress_report: Box<dyn ProgressReport>,
    ) -> Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TargetId(NodeIndex);

impl From<NodeIndex> for TargetId {
    fn from(value: NodeIndex) -> Self {
        Self(value)
    }
}

pub(crate) enum TargetSchedulerMessage {
    TargetScheduled {
        target_id: TargetId,
        target_metadata: Arc<TargetMetadata>,
    },
    TargetComplete {
        target_id: TargetId,
        target_metadata: Arc<TargetMetadata>,
    },
    TaskScheduled {
        target_id: TargetId,
        target_metadata: Arc<TargetMetadata>,
        task_id: TaskId,
        task_metadata: Arc<TaskMetadata>,
    },
    TaskStart {
        target_id: TargetId,
        #[allow(dead_code)]
        target_metadata: Arc<TargetMetadata>,
        task_id: TaskId,
        #[allow(dead_code)]
        task_metadata: Arc<TaskMetadata>,
    },
    TaskProgress {
        target_id: TargetId,
        #[allow(dead_code)]
        target_metadata: Arc<TargetMetadata>,
        #[allow(dead_code)]
        task_id: TaskId,
        #[allow(dead_code)]
        task_metadata: Arc<TaskMetadata>,
        position: u64,
        message: Option<String>,
    },
    TaskComplete {
        target_id: TargetId,
        target_metadata: Arc<TargetMetadata>,
        task_id: TaskId,
        task_metadata: Arc<TaskMetadata>,
        result: Result<()>,
    },
}

#[derive(Debug)]
pub(crate) enum TaskSchedulerMessage {
    Start {
        task_id: TaskId,
    },
    Progress {
        task_id: TaskId,
        message: Option<String>,
        position: u64,
    },
    Complete {
        task_id: TaskId,
        result: Result<()>,
    },
}

pub(crate) trait TaskScheduler {
    fn new(task_message_tx: Sender<TaskSchedulerMessage>) -> Self;
    fn dispatch(&self, task_id: TaskId, context: Arc<Mutex<TaskContext>>, task: Box<dyn Task>);
    fn wait_all(&self);
}

enum RayonTaskSchedulerMessage {
    Dispatch {
        task_id: TaskId,
        context: Arc<Mutex<TaskContext>>,
        task: Box<dyn Task>,
        task_message_tx: Sender<TaskSchedulerMessage>,
    },
    Stop,
}

pub(crate) struct RayonTaskScheduler {
    task_message_tx: Sender<TaskSchedulerMessage>,
    control_message_tx: Sender<RayonTaskSchedulerMessage>,
    control_message_rx: RefCell<Option<Receiver<RayonTaskSchedulerMessage>>>,
    join_handle: RefCell<Option<JoinHandle<Receiver<RayonTaskSchedulerMessage>>>>,
}

impl TaskScheduler for RayonTaskScheduler {
    fn new(task_message_tx: Sender<TaskSchedulerMessage>) -> Self {
        let (control_message_tx, control_message_rx) = mpsc::channel();
        Self {
            task_message_tx,
            control_message_tx,
            control_message_rx: RefCell::new(Some(control_message_rx)),
            join_handle: RefCell::new(None),
        }
    }

    fn dispatch(&self, task_id: TaskId, context: Arc<Mutex<TaskContext>>, task: Box<dyn Task>) {
        self.join_handle.borrow_mut().get_or_insert_with(|| {
            let control_message_rx = self.control_message_rx.borrow_mut().take().unwrap();
            std::thread::Builder::new()
                .name("rayon task scheduler control thread".to_owned())
                .spawn(move || {
                    // TODO: Handle the panic here better.
                    rayon::scope(move |s| {
                        while let Ok(control_message) = control_message_rx.recv() {
                            match control_message {
                                RayonTaskSchedulerMessage::Stop => break,
                                RayonTaskSchedulerMessage::Dispatch {
                                    task_id,
                                    context,
                                    task,
                                    task_message_tx,
                                } => s.spawn(move |_| {
                                    struct TaskSchedulerProgressReport {
                                        total: u64,
                                        position: RefCell<u64>,
                                        task_id: TaskId,
                                        task_message_tx: Sender<TaskSchedulerMessage>,
                                    }

                                    impl ProgressReport for TaskSchedulerProgressReport {
                                        fn set_message(&self, message: String) {
                                            self.task_message_tx
                                                .send(TaskSchedulerMessage::Progress {
                                                    task_id: self.task_id.clone(),
                                                    message: Some(message),
                                                    position: *self.position.borrow(),
                                                })
                                                .unwrap_or_else(|e| {
                                                    warn!(
                                                        "Failed to send the progress with \
                                                         message: {:?}",
                                                        e
                                                    )
                                                });
                                        }

                                        fn increase(&self, delta: u64) {
                                            *self.position.borrow_mut() += delta;
                                            self.task_message_tx
                                                .send(TaskSchedulerMessage::Progress {
                                                    task_id: self.task_id.clone(),
                                                    message: None,
                                                    position: *self.position.borrow(),
                                                })
                                                .unwrap_or_else(|e| {
                                                    warn!(
                                                        "Failed to send the progress with \
                                                         message: {:?}",
                                                        e
                                                    )
                                                });
                                        }

                                        fn finish(&self) {
                                            *self.position.borrow_mut() = self.total;
                                            self.task_message_tx
                                                .send(TaskSchedulerMessage::Progress {
                                                    task_id: self.task_id.clone(),
                                                    message: None,
                                                    position: *self.position.borrow(),
                                                })
                                                .unwrap_or_else(|e| {
                                                    warn!(
                                                        "Failed to send the progress with \
                                                         message: {:?}",
                                                        e
                                                    )
                                                });
                                        }
                                    }

                                    let progress_report = TaskSchedulerProgressReport {
                                        total: task.metadata().total_progress,
                                        position: RefCell::new(0),
                                        task_id: task_id.clone(),
                                        task_message_tx: task_message_tx.clone(),
                                    };

                                    task_message_tx
                                        .send(TaskSchedulerMessage::Start {
                                            task_id: task_id.clone(),
                                        })
                                        .unwrap_or_else(|_| {
                                            warn!(
                                                "Failed to send the start message for task {:?}",
                                                task_id
                                            )
                                        });
                                    let result = task.execute(context, Box::new(progress_report));
                                    task_message_tx
                                        .send(TaskSchedulerMessage::Complete { task_id, result })
                                        .unwrap_or_else(|e| {
                                            warn!("Failed to send the complete message: {:?}", e)
                                        });
                                }),
                            }
                        }
                        control_message_rx
                    })
                })
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to spawn rayon task scheduler control thread: {:?}",
                        e
                    )
                })
        });
        self.control_message_tx
            .send(RayonTaskSchedulerMessage::Dispatch {
                task_id,
                context,
                task,
                task_message_tx: self.task_message_tx.clone(),
            })
            .unwrap_or_else(|e| {
                panic!("Failed to dispatch the task to the control thread: {:?}", e)
            });
    }

    fn wait_all(&self) {
        let join_handle = match self.join_handle.borrow_mut().take() {
            Some(join_handle) => join_handle,
            None => return,
        };
        self.control_message_tx
            .send(RayonTaskSchedulerMessage::Stop)
            .unwrap_or_else(|_| warn!("The control thread loses connection."));
        let control_message_rx = join_handle.join().unwrap_or_else(|e| {
            error!("Failed to join the control thread.");
            // Continue the panic on the caller thread.
            std::panic::resume_unwind(e);
        });
        *self.control_message_rx.borrow_mut() = Some(control_message_rx);
    }
}

impl Drop for RayonTaskScheduler {
    fn drop(&mut self) {
        if self.join_handle.borrow().is_some() {
            self.control_message_tx
                .send(RayonTaskSchedulerMessage::Stop)
                .unwrap_or_else(|_| warn!("Failed to send stop request to the control thread."));
        }
    }
}

pub(crate) trait TypedTask: Send {
    type OutputType;

    fn metadata(&self) -> &TaskMetadata;
    fn execute(&self, progress_report: Box<dyn ProgressReport>) -> Result<Self::OutputType>;
    fn merge_output(&self, context: &mut TaskContext, output: Self::OutputType) -> Result<()>;
}

impl<T: TypedTask> Task for T {
    fn metadata(&self) -> &TaskMetadata {
        <Self as TypedTask>::metadata(self)
    }

    fn execute(
        &self,
        context: Arc<Mutex<TaskContext>>,
        progress_report: Box<dyn ProgressReport>,
    ) -> Result<()> {
        let task_name = self.metadata().name.as_deref().unwrap_or("(anonymous)");
        let output = <Self as TypedTask>::execute(self, progress_report)
            .with_context(|| format!("executing task: {}", task_name))?;
        self.merge_output(&mut context.lock().unwrap(), output)
            .with_context(|| format!("merge the output for task: {}", task_name))?;
        Ok(())
    }
}

pub(crate) trait SimpleTypedTask: Send {
    fn metadata(&self) -> &TaskMetadata;
    fn execute(&self, progress_report: Box<dyn ProgressReport>) -> Result<()>;
}

impl<T: SimpleTypedTask> TypedTask for T {
    type OutputType = ();

    fn metadata(&self) -> &TaskMetadata {
        <Self as SimpleTypedTask>::metadata(self)
    }

    fn execute(&self, progress_report: Box<dyn ProgressReport>) -> Result<()> {
        <Self as SimpleTypedTask>::execute(self, progress_report)
    }

    fn merge_output(&self, _: &mut TaskContext, _: Self::OutputType) -> Result<()> {
        Ok(())
    }
}

type CmdsBuilder = Box<dyn for<'a> Fn(&'a Shell) -> Vec<Cmd<'a>> + 'static + Send>;

pub(crate) struct CmdsTask {
    sh: Shell,
    metadata: TaskMetadata,
    cmds_builder: CmdsBuilder,
    cancellation_token: CancellationToken,
}

impl CmdsTask {
    pub fn new(
        name: Option<String>,
        cmds_builder: impl for<'a> Fn(&'a Shell) -> Vec<Cmd<'a>> + 'static + Send,
        cancellation_token: CancellationToken,
    ) -> Self {
        let sh = Shell::new().unwrap_or_else(|e| panic!("Failed to create shell: {:?}", e));
        let total_progress = cmds_builder(&sh).len().try_into().unwrap();
        Self {
            sh,
            metadata: TaskMetadata {
                name,
                total_progress,
            },
            cmds_builder: Box::new(cmds_builder),
            cancellation_token,
        }
    }
}

impl SimpleTypedTask for CmdsTask {
    fn metadata(&self) -> &TaskMetadata {
        &self.metadata
    }

    fn execute(&self, progress_report: Box<dyn ProgressReport>) -> Result<()> {
        self.cancellation_token.check_cancelled()?;
        for cmd in (self.cmds_builder)(&self.sh) {
            let cmd = cmd.quiet();
            progress_report.set_message(format!("{}", cmd));
            self.cancellation_token.check_cancelled()?;
            let command_name = cmd.to_string();
            let mut command: std::process::Command = cmd.into();
            let mut proc = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("spawn child process: {}", command_name))?;
            let mut poll_interval = Duration::from_micros(1);
            let status = loop {
                let wait_result = proc
                    .wait_timeout(poll_interval)
                    .with_context(|| format!("wait for child process: {}", command_name))?;
                if let Some(status) = wait_result {
                    break status;
                }
                if let Err(e) = self.cancellation_token.check_cancelled() {
                    if let Err(e) = proc.kill() {
                        error!("Failed to kill process `{}': {}", command_name, e);
                    }
                    return Err(e);
                }
                poll_interval = std::cmp::Ord::min(poll_interval * 2, Duration::from_millis(500));
            };
            if !status.success() {
                let output = proc
                    .wait_with_output()
                    .with_context(|| format!("wait with output for child: `{}'", command_name))?;
                let mut message = BufWriter::new(Vec::<u8>::new());
                writeln!(
                    message,
                    "command exited with non-zero code `{}`: {}",
                    command_name, status
                )
                .context("write general error message")?;
                writeln!(message).context("write a blank new line to the error message")?;
                writeln!(message, "stdout:").context("write stdout tag to error message")?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                writeln!(message, "{}", stdout).context("write stdout to error message")?;
                writeln!(message, "stderr:").context("write stderr tag to error message")?;
                let stderr = String::from_utf8_lossy(&output.stderr);
                writeln!(message, "{}", stderr).context("write stderr to error message")?;
                let message = message
                    .into_inner()
                    .context("unwrap the buffered writer for the error message")?;
                let message = String::from_utf8_lossy(&message).to_string();
                return Err(Error::msg(message));
            }

            progress_report.increase(1);
        }
        Ok(())
    }
}

pub(crate) struct TargetGraph(Graph<Box<dyn TargetNode>, ()>);

impl TargetGraph {
    pub fn create_from_targets(targets: impl IntoIterator<Item = Target>) -> Self {
        let mut target_graph = Graph::<Box<dyn TargetNode>, ()>::new();
        let mut vertices = BTreeMap::<Target, NodeIndex>::new();

        // BFS to build the graph.
        let mut visited = BTreeSet::<Target>::default();
        let mut targets = targets.into_iter().collect::<BTreeSet<_>>();
        while !targets.is_empty() {
            let mut next_targets = BTreeSet::new();
            for target in targets.iter() {
                visited.insert(target.clone());
                let entry = vertices.entry(target.clone());
                let target_node_idx = match entry {
                    btree_map::Entry::Vacant(entry) => {
                        let target_node = target_graph.add_node(target.clone().into());
                        debug!("Discover target node: {:?}", target);
                        *entry.insert(target_node)
                    }
                    btree_map::Entry::Occupied(entry) => *entry.get(),
                };
                let target_node = &target_graph[target_node_idx];
                let dependencies = target_node
                    .dependencies()
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                for dependency in dependencies.iter() {
                    let entry = vertices.entry(dependency.clone());
                    let dependency_idx = match entry {
                        btree_map::Entry::Vacant(entry) => {
                            let target_node = target_graph.add_node(dependency.clone().into());
                            debug!("Discover target node: {:?}", dependency);
                            *entry.insert(target_node)
                        }
                        btree_map::Entry::Occupied(entry) => *entry.get(),
                    };
                    target_graph.add_edge(dependency_idx, target_node_idx, ());
                    if !visited.contains(dependency) {
                        next_targets.insert(dependency.clone());
                    }
                }
            }
            targets = next_targets;
        }

        Self(target_graph)
    }

    pub fn execute<T: TaskScheduler>(
        &self,
        initial_context: super::Cli,
        mut target_scheduler_handler: impl FnMut(TargetSchedulerMessage) + 'static,
        cancellation_token_source: Arc<CancellationTokenSource>,
    ) -> Result<()> {
        struct Visitor<'a> {
            graph: &'a Graph<Box<dyn TargetNode>, ()>,
            to_visit: Vec<NodeIndex>,
            visited: FixedBitSet,
        }

        impl<'a> Visitor<'a> {
            fn new(graph: &'a Graph<Box<dyn TargetNode>, ()>) -> Self {
                // Initial with nodes without dependencies.
                let to_visit = graph
                    .node_identifiers()
                    .filter(|node_index| {
                        graph
                            .neighbors_directed(*node_index, Incoming)
                            .next()
                            .is_none()
                    })
                    .collect();
                Self {
                    graph,
                    to_visit,
                    visited: graph.visit_map(),
                }
            }

            fn mark_visited(&mut self, node: NodeIndex) {
                self.visited.visit(node);

                for neighbor in self.graph.neighbors(node) {
                    if self.visited.is_visited(&neighbor) {
                        continue;
                    }
                    if Reversed(self.graph)
                        .neighbors(neighbor)
                        .all(|node| self.visited.is_visited(&node))
                    {
                        self.to_visit.push(neighbor);
                    }
                }
            }

            // We don't implement the Iterator trait for this type, because once `mark_visited` is
            // called, whether next will return None may change.
            fn next(&mut self) -> Option<NodeIndex> {
                self.to_visit.pop()
            }

            fn completed(&self) -> bool {
                self.visited.count_ones(..) == self.graph.node_count()
            }
        }

        struct TargetInfo {
            metadata: Arc<TargetMetadata>,
            running_tasks: BTreeSet<TaskId>,
        }

        struct TargetScheduler<T: TaskScheduler> {
            context: Arc<Mutex<TaskContext>>,
            next_task_id: u32,
            task_scheduler: T,
            task_msg_rx: Receiver<TaskSchedulerMessage>,
            target_scheduler_handler: Box<dyn FnMut(TargetSchedulerMessage) + 'static>,
            task_info: BTreeMap<TaskId, Arc<TaskMetadata>>,
            target_info: BTreeMap<NodeIndex, TargetInfo>,
        }

        impl<T: TaskScheduler> TargetScheduler<T> {
            fn new(
                context: Arc<Mutex<TaskContext>>,
                target_scheduler_handler: impl FnMut(TargetSchedulerMessage) + 'static,
            ) -> Self {
                let (task_msg_tx, task_msg_rx) = mpsc::channel();
                Self {
                    context,
                    next_task_id: 0,
                    task_scheduler: T::new(task_msg_tx),
                    task_msg_rx,
                    target_scheduler_handler: Box::new(target_scheduler_handler),
                    task_info: Default::default(),
                    target_info: Default::default(),
                }
            }

            fn schedule_tasks(
                &mut self,
                target_id: NodeIndex,
                target_info: &mut TargetInfo,
                tasks: Vec<Box<dyn Task>>,
            ) {
                for task in tasks.into_iter() {
                    let task_id: TaskId = self.next_task_id.into();
                    let task_metadata = task.metadata().clone();
                    self.task_scheduler
                        .dispatch(task_id.clone(), Arc::clone(&self.context), task);
                    self.next_task_id += 1;
                    let task_metadata = Arc::new(task_metadata);
                    assert!(self
                        .task_info
                        .insert(task_id.clone(), Arc::clone(&task_metadata))
                        .is_none());
                    (self.target_scheduler_handler)(TargetSchedulerMessage::TaskScheduled {
                        target_id: target_id.into(),
                        target_metadata: Arc::clone(&target_info.metadata),
                        task_id: task_id.clone(),
                        task_metadata: Arc::clone(&task_metadata),
                    });
                    assert!(target_info.running_tasks.insert(task_id));
                }
            }

            fn find_target_id_by_task_id(&self, task_id: TaskId) -> Option<NodeIndex> {
                self.target_info
                    .iter()
                    .find(|(_, target)| target.running_tasks.contains(&task_id))
                    .map(|(target_id, _)| *target_id)
            }

            fn handle_task_scheduler_message(&mut self, task_message: TaskSchedulerMessage) {
                match task_message {
                    TaskSchedulerMessage::Start { task_id } => {
                        let target_id = self
                            .find_target_id_by_task_id(task_id.clone())
                            .unwrap_or_else(|| {
                                panic!(
                                    "Failed to find the target from the task {:?} when handling \
                                     task start message",
                                    task_id
                                )
                            });
                        (self.target_scheduler_handler)(TargetSchedulerMessage::TaskStart {
                            target_id: target_id.into(),
                            target_metadata: Arc::clone(&self.target_info[&target_id].metadata),
                            task_id: task_id.clone(),
                            task_metadata: Arc::clone(&self.task_info[&task_id]),
                        });
                    }
                    TaskSchedulerMessage::Progress {
                        task_id,
                        message,
                        position,
                    } => {
                        let target_id = self
                            .find_target_id_by_task_id(task_id.clone())
                            .unwrap_or_else(|| {
                                panic!(
                                    "Faied to find the target from the task {:?} when handling \
                                     task progress message",
                                    task_id
                                )
                            });
                        (self.target_scheduler_handler)(TargetSchedulerMessage::TaskProgress {
                            target_id: target_id.into(),
                            target_metadata: Arc::clone(&self.target_info[&target_id].metadata),
                            task_id: task_id.clone(),
                            task_metadata: Arc::clone(&self.task_info[&task_id]),
                            message,
                            position,
                        });
                    }
                    TaskSchedulerMessage::Complete { task_id, result } => {
                        let target_id = self
                            .find_target_id_by_task_id(task_id.clone())
                            .unwrap_or_else(|| {
                                panic!("Failed to find the target from the task: {:?}", task_id)
                            });
                        let target = self.target_info.get_mut(&target_id).unwrap_or_else(|| {
                            panic!(
                                "Failed to find the target status for target {:?}",
                                target_id
                            )
                        });
                        let target_metadata = Arc::clone(&target.metadata);
                        (self.target_scheduler_handler)(TargetSchedulerMessage::TaskComplete {
                            target_id: target_id.into(),
                            target_metadata: Arc::clone(&target_metadata),
                            task_id: task_id.clone(),
                            task_metadata: Arc::clone(&self.task_info[&task_id]),
                            result,
                        });
                        target.running_tasks.remove(&task_id);
                        if target.running_tasks.is_empty() {
                            (self.target_scheduler_handler)(
                                TargetSchedulerMessage::TargetComplete {
                                    target_id: target_id.into(),
                                    target_metadata,
                                },
                            );
                        }
                    }
                }
            }

            fn schedule_target(
                &mut self,
                target_idx: NodeIndex,
                target: &dyn TargetNode,
                cancellation_token: CancellationToken,
            ) {
                let target_metadata = Arc::new(target.metadata().clone());
                let mut target_info = TargetInfo {
                    running_tasks: BTreeSet::new(),
                    metadata: Arc::clone(&target_metadata),
                };
                (self.target_scheduler_handler)(TargetSchedulerMessage::TargetScheduled {
                    target_id: target_idx.into(),
                    target_metadata: Arc::clone(&target_metadata),
                });
                let tasks = target.create_tasks(Arc::clone(&self.context), cancellation_token);
                self.schedule_tasks(target_idx, &mut target_info, tasks);
                if target_info.running_tasks.is_empty() {
                    (self.target_scheduler_handler)(TargetSchedulerMessage::TargetComplete {
                        target_id: target_idx.into(),
                        target_metadata,
                    })
                }
                assert!(self.target_info.insert(target_idx, target_info).is_none());
            }

            fn drain_completed_targets(&mut self) -> Vec<NodeIndex> {
                let mut removed_targets = vec![];
                for (target_id, target) in self.target_info.iter() {
                    if !target.running_tasks.is_empty() {
                        continue;
                    }
                    removed_targets.push(*target_id);
                }
                for target_id in removed_targets.iter() {
                    self.target_info.remove(target_id);
                }
                removed_targets
            }

            fn has_pending_tasks(&self) -> bool {
                self.target_info
                    .values()
                    .any(|target| !target.running_tasks.is_empty())
            }

            fn wait_until_one_target_complete(&mut self) {
                while !self.target_info.is_empty()
                    && self
                        .target_info
                        .iter()
                        .all(|(_, target)| !target.running_tasks.is_empty())
                {
                    let task_message = match self.task_msg_rx.recv() {
                        Err(_) => {
                            assert!(
                                !self.has_pending_tasks(),
                                "The task scheduler loses the connection with remaining pending \
                                 tasks."
                            );
                            break;
                        }
                        Ok(message) => message,
                    };
                    self.handle_task_scheduler_message(task_message);
                }
            }

            fn wait_all(&self) {
                self.task_scheduler.wait_all();
            }

            fn has_pending_targets(&self) -> bool {
                !self.target_info.is_empty()
            }
        }

        let mut visitor = Visitor::new(&self.0);
        let failed: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
        let mut target_scheduler =
            TargetScheduler::<T>::new(Arc::new(Mutex::new(initial_context.into())), {
                let cancellation_token_source = Arc::clone(&cancellation_token_source);
                let failed = Rc::clone(&failed);
                move |message| {
                    if let TargetSchedulerMessage::TaskComplete { result: Err(_), .. } = &message {
                        *failed.borrow_mut() = true;
                        cancellation_token_source.cancel();
                    }
                    target_scheduler_handler(message);
                }
            });

        'outer: loop {
            let mut pending_targets = vec![];
            loop {
                for completed_target in target_scheduler.drain_completed_targets() {
                    visitor.mark_visited(completed_target);
                }
                if visitor.completed() {
                    break 'outer;
                }
                while let Some(node) = visitor.next() {
                    if cancellation_token_source.is_cancelled() {
                        break 'outer;
                    }
                    pending_targets.push((node, &self.0[node]));
                }

                if pending_targets.is_empty() {
                    if cancellation_token_source.is_cancelled() {
                        break 'outer;
                    }
                    assert!(target_scheduler.has_pending_targets());
                    target_scheduler.wait_until_one_target_complete();
                } else {
                    break;
                }
            }
            if cancellation_token_source.is_cancelled() {
                break;
            }
            for (node_index, target) in pending_targets {
                target_scheduler.schedule_target(
                    node_index,
                    target.as_ref(),
                    cancellation_token_source.token(),
                );
            }
            if cancellation_token_source.is_cancelled() {
                break;
            }
        }
        target_scheduler.wait_all();

        if *failed.borrow() {
            return Err(anyhow!("Tasks fail."));
        }

        Ok(())
    }
}
