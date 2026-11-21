use std::sync::Mutex;

use super::super::NativeModelClient;
use super::*;

/// Fails its first `failing` calls with `error`, then answers; records when each call came.
struct Flaky {
    failing: Mutex<usize>,
    error: ModelError,
    calls: Mutex<Vec<Instant>>,
}

impl Flaky {
    fn new(failing: usize, error: ModelError) -> Self {
        Self { failing: Mutex::new(failing), error, calls: Mutex::default() }
    }
}

impl Flaky {
    fn answer(&self) -> Result<(), ModelError> {
        self.calls.lock().unwrap().push(Instant::now());
        let mut failing = self.failing.lock().unwrap();
        if *failing == 0 {
            return Ok(());
        }
        *failing -= 1;
        Err(self.error.clone())
    }
}

impl NativeModelClient for Flaky {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.answer().map(|()| ModelResponse::text("ok"))
    }

    async fn window(&self, _: &str) -> Result<u64, ModelError> {
        self.answer().map(|()| 200_000)
    }
}

/// Never answers.
struct Stalled(Mutex<usize>);

impl NativeModelClient for Stalled {
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, ModelError> {
        *self.0.lock().unwrap() += 1;
        std::future::pending().await
    }

    async fn window(&self, _: &str) -> Result<u64, ModelError> {
        std::future::pending().await
    }
}

fn request() -> ModelRequest {
    ModelRequest {
        model: "claude-haiku-4-5".into(),
        agent: "a".into(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_tokens: 16,
        effort: None,
        compact: None,
    }
}

async fn ask(r: &Retry, client: &dyn super::super::ModelClient) -> Result<ModelResponse, ModelError> {
    r.call(client, request()).await
}

fn retry(deadline: Duration) -> Retry {
    Retry { attempts: 5, first: Duration::from_millis(10), deadline, ..Retry::default() }
}

#[tokio::test(start_paused = true)]
async fn after_5_failures_the_provider_gets_no_call_for_30_s_and_steps_wait_then_one_success_closes_it() {
    let flaky = Flaky::new(5, ModelError::Retryable("529".into()));
    let r = retry(Duration::from_secs(600));
    assert!(ask(&r, &flaky).await.is_err(), "the first step spends its 5 tries");
    let opened = Instant::now();
    assert!(ask(&r, &flaky).await.is_ok(), "the next step waits, not fails");
    let calls = flaky.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 6);
    assert!(calls[5] - opened >= breaker_open(), "no call while open");
    let before = Instant::now();
    ask(&r, &flaky).await.unwrap();
    assert_eq!(Instant::now(), before, "closed: the call goes at once");
}

fn breaker_open() -> Duration {
    shared::breaker::OPEN - Duration::from_millis(100)
}

#[tokio::test(start_paused = true)]
async fn a_retry_after_longer_than_30_s_holds_the_breaker_open() {
    let flaky = Flaky::new(5, ModelError::RetryAfter("429".into(), Duration::from_secs(90)));
    let r = retry(Duration::from_secs(1_000));
    let started = Instant::now();
    assert!(ask(&r, &flaky).await.is_err());
    let failed = Instant::now();
    ask(&r, &flaky).await.unwrap();
    assert!(flaky.calls.lock().unwrap()[5] - failed >= Duration::from_secs(89), "{:?}", started.elapsed());
}

#[tokio::test(start_paused = true)]
async fn a_step_whose_deadline_passes_while_open_fails_without_a_call() {
    let flaky = Flaky::new(5, ModelError::Retryable("503".into()));
    let r = retry(Duration::from_secs(600));
    assert!(ask(&r, &flaky).await.is_err());
    let short = Retry { deadline: Duration::from_secs(10), ..r.clone() };
    assert!(ask(&short, &flaky).await.is_err(), "ends as today, retries exhausted");
    assert_eq!(flaky.calls.lock().unwrap().len(), 5, "no call while open");
}

#[tokio::test(start_paused = true)]
async fn a_retry_after_past_what_an_instant_holds_ends_the_call_at_once() {
    let flaky = Flaky::new(1, ModelError::RetryAfter("429".into(), Duration::MAX));
    let started = Instant::now();
    assert!(ask(&retry(Duration::from_secs(600)), &flaky).await.is_err());
    assert_eq!(Instant::now(), started, "no wait");
    assert_eq!(flaky.calls.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_provider_that_never_answers_opens_its_breaker() {
    let stalled = Stalled(Mutex::new(0));
    let r = Retry { attempts: 1, ..retry(Duration::from_secs(1)) };
    for _ in 0..5 {
        assert!(ask(&r, &stalled).await.is_err(), "the step's deadline passed");
    }
    let before = Instant::now();
    assert!(ask(&r, &stalled).await.is_err());
    assert_eq!((*stalled.0.lock().unwrap(), Instant::now()), (5, before), "open: no call, no wait");
}

#[tokio::test(start_paused = true)]
async fn a_window_lookup_neither_opens_nor_closes_the_breaker() {
    let flaky = Flaky::new(5, ModelError::Retryable("529".into()));
    let r = retry(Duration::from_secs(600));
    assert!(r.window(&flaky, "claude-haiku-4-5").await.is_err(), "5 failed lookups");
    let before = Instant::now();
    assert!(ask(&r, &flaky).await.is_ok());
    assert_eq!(Instant::now(), before, "the breaker stayed closed");
    let opener = Flaky::new(5, ModelError::Retryable("529".into()));
    assert!(ask(&r, &opener).await.is_err(), "5 failed completions open it");
    assert_eq!(r.window(&opener, "claude-haiku-4-5").await.unwrap(), 200_000);
    let opened = Instant::now();
    assert!(ask(&r, &opener).await.is_ok());
    assert!(Instant::now() - opened >= breaker_open(), "a window lookup did not close it");
}
