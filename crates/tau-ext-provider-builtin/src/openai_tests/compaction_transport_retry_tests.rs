use super::*;

/// Exercise the real local-summary adapter, executor and delayed scheduler:
/// an upgrade request network failure must retry with the same logical owner,
/// complete once, and never dispatch a fallback HTTP summary.
#[test]
fn local_compaction_request_transport_retries_then_finishes_once() {
    let listener = path_std_net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut first, _) = listener.accept().expect("first upgrade");
        first
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            first.read_exact(&mut byte).expect("upgrade header");
            head.push(byte[0]);
            assert!(head.len() < 16_384, "bounded upgrade header");
        }
        assert!(head.starts_with(b"GET "));
        drop(first); // Fail request transport before response.create dispatch.
        let (second, _) = listener.accept().expect("retry upgrade");
        second
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut socket = tungstenite::accept(second).expect("upgrade");
        let tungstenite::Message::Text(request) = socket.read().expect("response.create") else {
            panic!("response.create request");
        };
        let request: serde_json::Value = serde_json::from_str(&request).expect("request JSON");
        assert_eq!(request["type"], "response.create");
        assert!(
            request["input"]
                .as_array()
                .expect("input")
                .iter()
                .all(|item| item["type"] != "compaction_trigger")
        );
        for event in [
            serde_json::json!({
                "type":"response.output_item.done", "output_index":0,
                "item":{"type":"message", "role":"assistant",
                    "content":[{"type":"output_text", "text":"Keep the invariant."}]}
            }),
            serde_json::json!({
                "type":"response.completed",
                "response":{"usage":{"input_tokens":100, "output_tokens":4}}
            }),
        ] {
            socket
                .send(tungstenite::Message::Text(event.to_string().into()))
                .expect("response");
        }
    });

    let clock = Arc::new(VirtualRetryClock::new(Instant::now()));
    let input = BlockingInput::default();
    let mut compact = prompt();
    compact.model = model_id(CHATGPT_PROVIDER_NAME, "gpt-6-luna");
    compact.operation = tau_proto::PromptOperation::StandaloneCompaction;
    compact
        .context
        .blocks
        .push(tau_proto::ContextBlock::UserInput(
            tau_proto::UserInputBlock {
                items: vec![ContextItem::CompactionTrigger],
            },
        ));
    input.push(encode_frames(&[live_event(
        11,
        Event::AgentPromptCreated(compact),
    )]));
    let attempts = Arc::new(AtomicUsize::new(0));
    let executor_attempts = Arc::clone(&attempts);
    let executor: PromptExecutor = Arc::new(move |mut execution| {
        executor_attempts.fetch_add(1, Ordering::SeqCst);
        let PromptBackend::Responses(config) = &mut execution.job.backend else {
            panic!("Codex route");
        };
        *config = config
            .clone()
            .with_test_base_url(format!("http://{address}/backend-api"));
        production_prompt_executor()(execution);
    });
    let profiles = profiles_with_chatgpt_auth(chatgpt_auth());
    let prompt_profiles = profiles.clone();
    let writer = SharedWriter::default();
    let output = writer.clone();
    let runtime_input = input.clone();
    let runtime_clock: Arc<dyn RetryClock> = clock.clone();
    let runtime = thread::spawn(move || {
        run_inner_with_executors_and_clock(
            runtime_input,
            writer,
            profiles,
            move |_| prompt_profiles.clone(),
            1,
            RuntimeExecutors {
                prompt: executor,
                prewarm: production_prewarm_executor(),
                retry_clock: runtime_clock,
            },
        )
        .expect("run production executor");
    });
    wait_for_runtime_frames(&output, |frames| {
        frames.iter().any(|frame| {
            matches!(
                input_event(frame),
                Some(Event::ProviderResponseUpdatedReported(update))
                    if update.status.as_ref().is_some_and(|status| status.retry.is_some())
            )
        })
    });
    clock.advance(Duration::from_secs(3600));
    let frames = wait_for_runtime_frames(&output, |frames| {
        frames.iter().any(|frame| {
            matches!(
                input_event(frame),
                Some(Event::ProviderResponseFinishedReported(_))
            )
        })
    });
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        frames
            .iter()
            .filter(|frame| matches!(
                input_event(frame),
                Some(Event::ProviderPromptSubmittedReported(_))
            ))
            .count(),
        1
    );
    let terminals: Vec<_> = frames
        .iter()
        .filter_map(|frame| match input_event(frame) {
            Some(Event::ProviderResponseFinishedReported(finished)) => Some(finished),
            _ => None,
        })
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].provider_attempt.get(), 2);
    assert!(terminals[0].error.is_none(), "{:?}", terminals[0].error);
    assert_eq!(terminals[0].output_items.len(), 1);
    input.close();
    runtime.join().expect("runtime join");
    server.join().expect("server join");
}
