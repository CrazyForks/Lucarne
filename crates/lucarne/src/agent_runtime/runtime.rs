use async_trait::async_trait;
use smol_str::SmolStr;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex, RwLock as StdRwLock,
    },
};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, trace, warn};

use super::{
    AgentCapabilities, AgentError, AgentErrorKind, AgentEventStream, AgentInput, AgentProvider,
    AgentSession, CommandId, CommandRejectedEvent, InstanceId, InterventionResponse, OpenSession,
    ResumeSession, RuntimeBusFilter, RuntimeBusOutput, RuntimeBusStream, RuntimeCommand,
    SessionClosedEvent, SessionId, SessionOpenedEvent,
};
use crate::ProviderId;

const COMMAND_BUFFER: usize = 64;
const EVENT_BUFFER: usize = 64;
const CLOSED_REASON: &str = "session stream closed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDescriptor {
    pub id: ProviderId,
    pub label: SmolStr,
    pub binary: SmolStr,
    pub capabilities: AgentCapabilities,
}

#[async_trait]
pub trait RuntimeBus: Send + Sync {
    async fn command(&self, cmd: RuntimeCommand) -> Result<(), AgentError>;
    async fn take_events(&self) -> Result<RuntimeBusStream, AgentError>;
}

pub struct AgentRuntime {
    state: Arc<RuntimeState>,
    bus: Arc<SharedRuntimeBus>,
}

struct RoutedCommand {
    command: RuntimeCommand,
    applied_tx: Option<oneshot::Sender<()>>,
}

impl AgentRuntime {
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_BUFFER);
        let (event_tx, event_rx) = mpsc::channel(EVENT_BUFFER);
        let state = Arc::new(RuntimeState {
            providers: StdRwLock::new(BTreeMap::new()),
            sessions: StdRwLock::new(HashMap::new()),
            filter: StdRwLock::new(RuntimeBusFilter::default()),
            event_tx,
        });
        debug!(
            target: "lucarne::agent_runtime",
            command_buffer = COMMAND_BUFFER,
            event_buffer = EVENT_BUFFER,
            "initializing agent runtime"
        );
        tokio::spawn(run_command_router(Arc::clone(&state), command_rx));

        Self {
            state,
            bus: Arc::new(SharedRuntimeBus {
                command_tx,
                events_rx: StdMutex::new(Some(event_rx)),
            }),
        }
    }

    pub fn register(&self, provider: Arc<dyn AgentProvider>) {
        let descriptor = AgentDescriptor {
            id: provider.id(),
            label: SmolStr::new(provider.label()),
            binary: SmolStr::new(provider.binary()),
            capabilities: provider.capabilities(),
        };
        info!(
            target: "lucarne::agent_runtime",
            provider_id = %descriptor.id,
            provider_label = %descriptor.label,
            "registering provider"
        );
        self.state
            .providers
            .write()
            .expect("provider registry lock")
            .insert(
                descriptor.id,
                RegisteredProvider {
                    descriptor,
                    provider,
                },
            );
    }

    pub fn provider(&self, id: &str) -> Option<Arc<dyn AgentProvider>> {
        self.state
            .providers
            .read()
            .expect("provider registry lock")
            .iter()
            .find(|(provider_id, _)| provider_id.as_str() == id)
            .map(|(_, registered)| registered)
            .map(|registered| Arc::clone(&registered.provider))
    }

    pub fn providers(&self) -> Vec<AgentDescriptor> {
        self.state
            .providers
            .read()
            .expect("provider registry lock")
            .values()
            .map(|registered| registered.descriptor.clone())
            .collect()
    }

    pub fn bus(&self) -> Arc<dyn RuntimeBus> {
        self.bus.clone()
    }

    pub async fn open(
        &self,
        provider_id: &str,
        req: OpenSession,
    ) -> Result<Box<dyn AgentSession>, AgentError> {
        info!(
            target: "lucarne::agent_runtime",
            provider_id,
            model = req.model.as_deref().unwrap_or("-"),
            cwd = req.cwd.as_deref().unwrap_or("-"),
            has_initial_input = req.initial_input.is_some(),
            "opening session"
        );
        let provider = self
            .provider(provider_id)
            .ok_or_else(|| unknown_provider(provider_id))?;
        let session = provider.open(req).await?;
        info!(
            target: "lucarne::agent_runtime",
            provider_id,
            instance_id = %session.instance_id().0,
            session_id = %session.id().0,
            "session opened"
        );
        Ok(session)
    }

    pub async fn resume(
        &self,
        provider_id: &str,
        req: ResumeSession,
    ) -> Result<Box<dyn AgentSession>, AgentError> {
        info!(
            target: "lucarne::agent_runtime",
            provider_id,
            session_ref = %req.session_ref.0,
            "resuming session"
        );
        let provider = self
            .provider(provider_id)
            .ok_or_else(|| unknown_provider(provider_id))?;
        let session = provider.resume(req).await?;
        info!(
            target: "lucarne::agent_runtime",
            provider_id,
            instance_id = %session.instance_id().0,
            session_id = %session.id().0,
            "session resumed"
        );
        Ok(session)
    }

    /// Register the default protocol providers that lucarne ships.
    /// Missing binaries are skipped silently; the runtime simply won't
    /// offer that provider via [`open`] / [`resume`].
    ///
    /// This is a convenience for front-ends (bots, CLIs) that want
    /// "all available local agents" in one call. Applications with
    /// custom binary paths should call [`register`] directly.
    pub fn register_defaults(&self) {
        self.register_default_adapters(crate::adapters::default_adapters());
    }

    pub fn register_defaults_filtered(&self, enabled_ids: &[String]) {
        self.register_default_adapters(crate::adapters::default_adapters_for_provider_ids(
            enabled_ids,
        ));
    }

    fn register_default_adapters(&self, adapters: Vec<Arc<crate::adapter::ProtocolAdapter>>) {
        for adapter in adapters {
            match super::ProtocolProvider::new(adapter) {
                Ok(p) => {
                    info!(
                        target: "lucarne::agent_runtime",
                        provider_id = %p.id(),
                        "registering default provider"
                    );
                    self.register(Arc::new(p));
                }
                Err(e) => tracing::debug!(
                    target: "lucarne::agent_runtime",
                    "skipping default provider: {e}"
                ),
            }
        }
    }
}

struct RegisteredProvider {
    descriptor: AgentDescriptor,
    provider: Arc<dyn AgentProvider>,
}

struct SharedRuntimeBus {
    command_tx: mpsc::Sender<RoutedCommand>,
    events_rx: StdMutex<Option<RuntimeBusStream>>,
}

#[async_trait]
impl RuntimeBus for SharedRuntimeBus {
    async fn command(&self, cmd: RuntimeCommand) -> Result<(), AgentError> {
        trace!(
            target: "lucarne::agent_runtime",
            command = runtime_command_name(&cmd),
            "sending runtime command"
        );
        let (applied_tx, applied_rx) = match cmd {
            RuntimeCommand::UpdateFilter { .. } => {
                let (tx, rx) = oneshot::channel();
                (Some(tx), Some(rx))
            }
            _ => (None, None),
        };

        self.command_tx
            .send(RoutedCommand {
                command: cmd,
                applied_tx,
            })
            .await
            .map_err(|_| invalid_state("runtime bus command router is not running"))?;

        if let Some(applied_rx) = applied_rx {
            applied_rx.await.map_err(|_| {
                invalid_state("runtime bus command router stopped before applying filter")
            })?;
        }

        Ok(())
    }

    async fn take_events(&self) -> Result<RuntimeBusStream, AgentError> {
        debug!(target: "lucarne::agent_runtime", "taking runtime bus event stream");
        self.events_rx
            .lock()
            .expect("runtime bus event stream lock")
            .take()
            .ok_or_else(|| invalid_state("runtime bus event stream already taken"))
    }
}

struct RuntimeState {
    providers: StdRwLock<BTreeMap<ProviderId, RegisteredProvider>>,
    sessions: StdRwLock<HashMap<InstanceId, Arc<ManagedSessionInner>>>,
    filter: StdRwLock<RuntimeBusFilter>,
    event_tx: mpsc::Sender<RuntimeBusOutput>,
}

struct ManagedSessionInner {
    provider_id: ProviderId,
    instance_id: InstanceId,
    session_id: SessionId,
    command_tx: mpsc::UnboundedSender<ManagedCommand>,
    pump_tx: mpsc::UnboundedSender<PumpControl>,
    closing: Arc<AtomicBool>,
}

struct PendingSession {
    managed: Arc<ManagedSessionInner>,
    ready_tx: Option<oneshot::Sender<()>>,
}

enum ManagedCommand {
    Submit {
        input: AgentInput,
    },
    Interrupt,
    Resolve {
        req_id: SmolStr,
        response: InterventionResponse,
    },
    Close,
}

enum PumpControl {
    UpdateFilter {
        filter: RuntimeBusFilter,
        applied_tx: oneshot::Sender<()>,
    },
}

impl PendingSession {
    fn start(mut self) {
        if let Some(ready_tx) = self.ready_tx.take() {
            let _ = ready_tx.send(());
        }
    }
}

async fn run_command_router(
    state: Arc<RuntimeState>,
    mut command_rx: mpsc::Receiver<RoutedCommand>,
) {
    info!(target: "lucarne::agent_runtime", "command router started");
    while let Some(routed) = command_rx.recv().await {
        trace!(
            target: "lucarne::agent_runtime",
            command = runtime_command_name(&routed.command),
            "routing runtime command"
        );
        match routed.command {
            RuntimeCommand::Open {
                command_id,
                provider_id,
                req,
            } => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let result = open_from_bus(Arc::clone(&state), provider_id, req).await;
                    match result {
                        Ok(session) => {
                            state
                                .emit(RuntimeBusOutput::SessionOpened(SessionOpenedEvent {
                                    command_id,
                                    instance_id: session.managed.instance_id.clone(),
                                    provider_id: session.managed.provider_id,
                                    session_id: session.managed.session_id.clone(),
                                }))
                                .await;
                            session.start();
                        }
                        Err(err) => {
                            reject_command(&state, Some(command_id), None, None, err.message).await;
                        }
                    }
                });
            }
            RuntimeCommand::Resume {
                command_id,
                provider_id,
                req,
            } => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let result = resume_from_bus(Arc::clone(&state), provider_id, req).await;
                    match result {
                        Ok(session) => {
                            state
                                .emit(RuntimeBusOutput::SessionOpened(SessionOpenedEvent {
                                    command_id,
                                    instance_id: session.managed.instance_id.clone(),
                                    provider_id: session.managed.provider_id,
                                    session_id: session.managed.session_id.clone(),
                                }))
                                .await;
                            session.start();
                        }
                        Err(err) => {
                            reject_command(&state, Some(command_id), None, None, err.message).await;
                        }
                    }
                });
            }
            RuntimeCommand::Submit { instance_id, input } => {
                enqueue_session_command(
                    Arc::clone(&state),
                    instance_id,
                    None,
                    ManagedCommand::Submit { input },
                )
                .await;
            }
            RuntimeCommand::Interrupt { instance_id } => {
                enqueue_session_command(
                    Arc::clone(&state),
                    instance_id,
                    None,
                    ManagedCommand::Interrupt,
                )
                .await;
            }
            RuntimeCommand::Resolve {
                instance_id,
                req_id,
                response,
            } => {
                enqueue_session_command(
                    Arc::clone(&state),
                    instance_id,
                    None,
                    ManagedCommand::Resolve { req_id, response },
                )
                .await;
            }
            RuntimeCommand::Close { instance_id } => {
                enqueue_session_command(
                    Arc::clone(&state),
                    instance_id,
                    None,
                    ManagedCommand::Close,
                )
                .await;
            }
            RuntimeCommand::UpdateFilter { filter } => {
                apply_runtime_filter(Arc::clone(&state), filter, routed.applied_tx).await;
            }
        }
    }
}

async fn apply_runtime_filter(
    state: Arc<RuntimeState>,
    filter: RuntimeBusFilter,
    applied_tx: Option<oneshot::Sender<()>>,
) {
    debug!(
        target: "lucarne::agent_runtime",
        session_lifecycle = filter.session_lifecycle,
        user_messages = filter.user_messages,
        assistant_messages = filter.assistant_messages,
        reasoning = filter.reasoning,
        tool_calls = filter.tool_calls,
        tool_results = filter.tool_results,
        usage = filter.usage,
        intervention_requests = filter.intervention_requests,
        "applying runtime filter"
    );
    *state.filter.write().expect("runtime filter lock") = filter;
    let sessions = state
        .sessions
        .read()
        .expect("runtime session registry lock")
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let mut applied_rxs = Vec::new();

    for session in sessions {
        let (session_applied_tx, session_applied_rx) = oneshot::channel();
        if session
            .pump_tx
            .send(PumpControl::UpdateFilter {
                filter,
                applied_tx: session_applied_tx,
            })
            .is_ok()
        {
            applied_rxs.push(session_applied_rx);
        }
    }

    for applied_rx in applied_rxs {
        let _ = applied_rx.await;
    }

    if let Some(applied_tx) = applied_tx {
        let _ = applied_tx.send(());
    }
}

async fn open_from_bus(
    state: Arc<RuntimeState>,
    provider_id: ProviderId,
    req: OpenSession,
) -> Result<PendingSession, AgentError> {
    debug!(
        target: "lucarne::agent_runtime",
        provider_id = %provider_id,
        model = req.model.as_deref().unwrap_or("-"),
        cwd = req.cwd.as_deref().unwrap_or("-"),
        "opening session from runtime bus"
    );
    let provider = state
        .provider(provider_id)
        .ok_or_else(|| unknown_provider(provider_id.as_str()))?;
    let session = provider.open(req).await?;
    prepare_live_session(state, session).await
}

async fn resume_from_bus(
    state: Arc<RuntimeState>,
    provider_id: ProviderId,
    req: ResumeSession,
) -> Result<PendingSession, AgentError> {
    debug!(
        target: "lucarne::agent_runtime",
        provider_id = %provider_id,
        session_ref = %req.session_ref.0,
        "resuming session from runtime bus"
    );
    let provider = state
        .provider(provider_id)
        .ok_or_else(|| unknown_provider(provider_id.as_str()))?;
    let session = provider.resume(req).await?;
    prepare_live_session(state, session).await
}

async fn prepare_live_session(
    state: Arc<RuntimeState>,
    session: Box<dyn AgentSession>,
) -> Result<PendingSession, AgentError> {
    let session: Arc<dyn AgentSession> = session.into();
    let filter = *state.filter.read().expect("runtime filter lock");
    let filters_upstream = session.update_runtime_filter(filter).await?;
    let provider_id = session.provider_id();
    let instance_id = session.instance_id().clone();
    let session_id = session.id().clone();
    let source_events = session.take_events().await?;
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (pump_tx, pump_rx) = mpsc::unbounded_channel();
    let closing = Arc::new(AtomicBool::new(false));

    let managed = Arc::new(ManagedSessionInner {
        provider_id,
        instance_id: instance_id.clone(),
        session_id: session_id.clone(),
        command_tx,
        pump_tx,
        closing: Arc::clone(&closing),
    });
    info!(
        target: "lucarne::agent_runtime",
        provider_id = %provider_id,
        instance_id = %instance_id.0,
        session_id = %session_id.0,
        filters_upstream,
        "prepared live session"
    );

    state
        .sessions
        .write()
        .expect("runtime session registry lock")
        .insert(instance_id.clone(), Arc::clone(&managed));

    tokio::spawn(run_session_commands(
        Arc::clone(&state),
        instance_id.clone(),
        session_id.clone(),
        Arc::clone(&session),
        closing,
        command_rx,
    ));

    let (ready_tx, ready_rx) = oneshot::channel();
    tokio::spawn(pump_session_events(
        state,
        instance_id,
        provider_id,
        session_id,
        Arc::clone(&session),
        source_events,
        Some(filter),
        filters_upstream,
        pump_rx,
        ready_rx,
    ));

    Ok(PendingSession {
        managed,
        ready_tx: Some(ready_tx),
    })
}

async fn pump_session_events(
    state: Arc<RuntimeState>,
    instance_id: InstanceId,
    provider_id: ProviderId,
    session_id: SessionId,
    session: Arc<dyn AgentSession>,
    mut source_events: AgentEventStream,
    mut filter: Option<RuntimeBusFilter>,
    filters_upstream: bool,
    mut control_rx: mpsc::UnboundedReceiver<PumpControl>,
    ready_rx: oneshot::Receiver<()>,
) {
    debug!(
        target: "lucarne::agent_runtime",
        provider_id = %provider_id,
        instance_id = %instance_id.0,
        session_id = %session_id.0,
        filters_upstream,
        "session event pump started"
    );
    let mut ready = false;
    let mut ready_rx = ready_rx;
    let mut source_closed = false;
    let mut pending_outputs: VecDeque<RuntimeBusOutput> = VecDeque::new();
    loop {
        if ready && source_closed && pending_outputs.is_empty() {
            break;
        }

        if ready {
            if let Some(output) = pending_outputs.pop_front() {
                tokio::select! {
                    biased;
                    Some(control) = control_rx.recv() => {
                        pending_outputs.push_front(output);
                        apply_pump_control(
                            &instance_id,
                            provider_id,
                            &session_id,
                            session.as_ref(),
                            &mut source_events,
                            &mut filter,
                            filters_upstream,
                            &mut pending_outputs,
                            control,
                        ).await;
                    }
                    result = state.event_tx.send(output.clone()) => {
                        let _ = result;
                    }
                }
                continue;
            }
        }

        tokio::select! {
            biased;
            ready_result = &mut ready_rx, if !ready => {
                if ready_result.is_err() {
                    warn!(
                        target: "lucarne::agent_runtime",
                        provider_id = %provider_id,
                        instance_id = %instance_id.0,
                        session_id = %session_id.0,
                        "session readiness channel closed before ready"
                    );
                    return;
                }
                ready = true;
            }
            Some(control) = control_rx.recv() => {
                apply_pump_control(
                    &instance_id,
                    provider_id,
                    &session_id,
                    session.as_ref(),
                    &mut source_events,
                    &mut filter,
                    filters_upstream,
                    &mut pending_outputs,
                    control,
                ).await;
            }
            maybe_event = source_events.recv(), if !source_closed => match maybe_event {
                Some(event) => {
                    queue_session_event(
                        &instance_id,
                        provider_id,
                        &session_id,
                        filter.as_ref(),
                        &mut pending_outputs,
                        event,
                    );
                }
                None => {
                    source_closed = true;
                    debug!(
                        target: "lucarne::agent_runtime",
                        provider_id = %provider_id,
                        instance_id = %instance_id.0,
                        session_id = %session_id.0,
                        "source event stream closed"
                    );
                }
            }
        }
    }

    state
        .sessions
        .write()
        .expect("runtime session registry lock")
        .remove(&instance_id);
    let reason = session
        .observed_close_reason()
        .await
        .unwrap_or_else(|| CLOSED_REASON.into());
    info!(
        target: "lucarne::agent_runtime",
        provider_id = %provider_id,
        instance_id = %instance_id.0,
        session_id = %session_id.0,
        reason = %reason,
        "session event pump ended"
    );
    state
        .emit(RuntimeBusOutput::SessionClosed(SessionClosedEvent {
            instance_id,
            provider_id,
            session_id,
            reason,
        }))
        .await;
}

async fn apply_pump_control(
    instance_id: &InstanceId,
    provider_id: ProviderId,
    session_id: &SessionId,
    session: &dyn AgentSession,
    source_events: &mut AgentEventStream,
    filter: &mut Option<RuntimeBusFilter>,
    filters_upstream: bool,
    pending_outputs: &mut VecDeque<RuntimeBusOutput>,
    control: PumpControl,
) {
    match control {
        PumpControl::UpdateFilter {
            filter: next_filter,
            applied_tx,
        } => {
            debug!(
                target: "lucarne::agent_runtime",
                provider_id = %provider_id,
                instance_id = %instance_id.0,
                session_id = %session_id.0,
                "updating session pump filter"
            );
            let upstream_controls_filter = session
                .update_runtime_filter(next_filter)
                .await
                .unwrap_or(false);
            while let Ok(event) = source_events.try_recv() {
                queue_session_event(
                    instance_id,
                    provider_id,
                    session_id,
                    filter.as_ref(),
                    pending_outputs,
                    event,
                );
            }
            *filter = if filters_upstream || upstream_controls_filter {
                None
            } else {
                Some(next_filter)
            };
            let _ = applied_tx.send(());
        }
    }
}

fn queue_session_event(
    instance_id: &InstanceId,
    provider_id: ProviderId,
    session_id: &SessionId,
    filter: Option<&RuntimeBusFilter>,
    pending_outputs: &mut VecDeque<RuntimeBusOutput>,
    event: super::Event,
) {
    if filter.map_or(true, |filter| event.matches_filter(filter)) {
        trace!(
            target: "lucarne::agent_runtime",
            provider_id = %provider_id,
            instance_id = %instance_id.0,
            session_id = %session_id.0,
            event = agent_event_name(&event),
            "queueing runtime event"
        );
        pending_outputs.push_back(RuntimeBusOutput::Event(super::RuntimeBusEvent {
            instance_id: instance_id.clone(),
            provider_id,
            session_id: session_id.clone(),
            event,
        }));
    } else {
        trace!(
            target: "lucarne::agent_runtime",
            provider_id = %provider_id,
            instance_id = %instance_id.0,
            session_id = %session_id.0,
            event = agent_event_name(&event),
            "filtered runtime event"
        );
    }
}

async fn run_session_commands(
    state: Arc<RuntimeState>,
    instance_id: InstanceId,
    session_id: SessionId,
    session: Arc<dyn AgentSession>,
    closing: Arc<AtomicBool>,
    mut command_rx: mpsc::UnboundedReceiver<ManagedCommand>,
) {
    debug!(
        target: "lucarne::agent_runtime",
        instance_id = %instance_id.0,
        session_id = %session_id.0,
        "session command loop started"
    );
    while let Some(command) = command_rx.recv().await {
        let command_name = managed_command_name(&command);
        trace!(
            target: "lucarne::agent_runtime",
            instance_id = %instance_id.0,
            session_id = %session_id.0,
            command = command_name,
            "running managed command"
        );
        let stop_after = matches!(command, ManagedCommand::Close);
        if stop_after {
            closing.store(true, Ordering::Relaxed);
        }
        let result = match command {
            ManagedCommand::Submit { input } => session.submit(input).await,
            ManagedCommand::Interrupt => session.interrupt().await,
            ManagedCommand::Resolve { req_id, response } => {
                session.resolve(&req_id, response).await
            }
            ManagedCommand::Close => session.close().await,
        };

        if let Err(err) = result {
            if stop_after {
                closing.store(false, Ordering::Relaxed);
            }
            warn!(
                target: "lucarne::agent_runtime",
                instance_id = %instance_id.0,
                session_id = %session_id.0,
                command = command_name,
                error = %err.message,
                "managed command failed"
            );
            reject_command(
                &state,
                None,
                Some(instance_id.clone()),
                Some(session_id.clone()),
                err.message,
            )
            .await;
            continue;
        }

        if stop_after {
            info!(
                target: "lucarne::agent_runtime",
                instance_id = %instance_id.0,
                session_id = %session_id.0,
                "managed close completed"
            );
            break;
        }
    }
}

async fn enqueue_session_command(
    state: Arc<RuntimeState>,
    instance_id: InstanceId,
    command_id: Option<CommandId>,
    command: ManagedCommand,
) {
    let session = state
        .sessions
        .read()
        .expect("runtime session registry lock")
        .get(&instance_id)
        .cloned();
    let Some(session) = session else {
        warn!(
            target: "lucarne::agent_runtime",
            instance_id = %instance_id.0,
            command_id = command_id.as_ref().map(|id| id.0.as_str()).unwrap_or("-"),
            "rejecting command for unknown instance"
        );
        reject_command(
            &state,
            command_id,
            Some(instance_id.clone()),
            None,
            format!("unknown instance {:?}", instance_id.0),
        )
        .await;
        return;
    };

    let is_close = matches!(command, ManagedCommand::Close);
    if session.closing.load(Ordering::Relaxed) {
        warn!(
            target: "lucarne::agent_runtime",
            instance_id = %instance_id.0,
            session_id = %session.session_id.0,
            command = managed_command_name(&command),
            "rejecting command because session is closing"
        );
        reject_command(
            &state,
            command_id,
            Some(instance_id),
            Some(session.session_id.clone()),
            "session is closing",
        )
        .await;
        return;
    }
    if is_close {
        session.closing.store(true, Ordering::Relaxed);
    }
    trace!(
        target: "lucarne::agent_runtime",
        instance_id = %instance_id.0,
        session_id = %session.session_id.0,
        command = managed_command_name(&command),
        "enqueueing managed command"
    );

    if session.command_tx.send(command).is_err() {
        warn!(
            target: "lucarne::agent_runtime",
            instance_id = %instance_id.0,
            session_id = %session.session_id.0,
            "managed command loop is not running"
        );
        reject_command(
            &state,
            command_id,
            Some(instance_id),
            Some(session.session_id.clone()),
            "session command loop is not running",
        )
        .await;
    }
}

impl RuntimeState {
    fn provider(&self, id: ProviderId) -> Option<Arc<dyn AgentProvider>> {
        self.providers
            .read()
            .expect("provider registry lock")
            .get(&id)
            .map(|registered| Arc::clone(&registered.provider))
    }

    async fn emit(&self, output: RuntimeBusOutput) {
        let should_emit = match &output {
            RuntimeBusOutput::SessionOpened(_) | RuntimeBusOutput::SessionClosed(_) => {
                self.filter
                    .read()
                    .expect("runtime filter lock")
                    .session_lifecycle
            }
            RuntimeBusOutput::Event(_) | RuntimeBusOutput::CommandRejected(_) => true,
        };
        if should_emit {
            trace!(
                target: "lucarne::agent_runtime",
                output = runtime_output_name(&output),
                "emitting runtime bus output"
            );
            let _ = self.event_tx.send(output).await;
        } else {
            trace!(
                target: "lucarne::agent_runtime",
                output = runtime_output_name(&output),
                "dropping runtime bus output due to filter"
            );
        }
    }
}

async fn reject_command(
    state: &RuntimeState,
    command_id: Option<CommandId>,
    instance_id: Option<InstanceId>,
    session_id: Option<SessionId>,
    message: impl Into<SmolStr>,
) {
    let message = message.into();
    warn!(
        target: "lucarne::agent_runtime",
        command_id = command_id.as_ref().map(|id| id.0.as_str()).unwrap_or("-"),
        instance_id = instance_id.as_ref().map(|id| id.0.as_str()).unwrap_or("-"),
        session_id = session_id.as_ref().map(|id| id.0.as_str()).unwrap_or("-"),
        message = %message,
        "runtime command rejected"
    );
    state
        .emit(RuntimeBusOutput::CommandRejected(CommandRejectedEvent {
            command_id,
            session_id,
            instance_id,
            message,
        }))
        .await;
}

fn runtime_command_name(command: &RuntimeCommand) -> &'static str {
    match command {
        RuntimeCommand::Open { .. } => "open",
        RuntimeCommand::Resume { .. } => "resume",
        RuntimeCommand::Submit { .. } => "submit",
        RuntimeCommand::Interrupt { .. } => "interrupt",
        RuntimeCommand::Resolve { .. } => "resolve",
        RuntimeCommand::Close { .. } => "close",
        RuntimeCommand::UpdateFilter { .. } => "update_filter",
    }
}

fn managed_command_name(command: &ManagedCommand) -> &'static str {
    match command {
        ManagedCommand::Submit { .. } => "submit",
        ManagedCommand::Interrupt => "interrupt",
        ManagedCommand::Resolve { .. } => "resolve",
        ManagedCommand::Close => "close",
    }
}

fn runtime_output_name(output: &RuntimeBusOutput) -> &'static str {
    match output {
        RuntimeBusOutput::SessionOpened(_) => "session_opened",
        RuntimeBusOutput::SessionClosed(_) => "session_closed",
        RuntimeBusOutput::Event(_) => "event",
        RuntimeBusOutput::CommandRejected(_) => "command_rejected",
    }
}

fn agent_event_name(event: &super::Event) -> &'static str {
    match event {
        super::Event::Message(_) => "message",
        super::Event::Reasoning(_) => "reasoning",
        super::Event::ToolCall(_) => "tool_call",
        super::Event::ToolResult(_) => "tool_result",
        super::Event::Usage(_) => "usage",
        super::Event::CommandResult(_) => "command_result",
        super::Event::InterventionRequest(_) => "intervention_request",
        super::Event::TurnCompleted(_) => "turn_completed",
        super::Event::TurnFailed(_) => "turn_failed",
    }
}

fn unknown_provider(provider_id: &str) -> AgentError {
    AgentError {
        kind: AgentErrorKind::Unsupported,
        message: format!("unknown provider {:?}", provider_id).into(),
    }
}

fn invalid_state(message: impl Into<String>) -> AgentError {
    AgentError {
        kind: AgentErrorKind::InvalidState,
        message: message.into().into(),
    }
}
