use async_trait::async_trait;
use std::sync::Mutex;
use test_api::{IntegrationTestService, aura_integration_test};

static EVENTS: Mutex<Vec<&str>> = Mutex::new(Vec::new());

fn record(event: &'static str) {
    EVENTS.lock().unwrap().push(event);
}

struct OrderedService(&'static str);

#[async_trait]
impl IntegrationTestService for OrderedService {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        if self.0 == "a" {
            EVENTS.lock().unwrap().clear();
        }
        record(match self.0 {
            "a" => "setup-a",
            "b" => "setup-b",
            _ => unreachable!(),
        });
    }

    async fn tear_down(&self) {
        let mut events = EVENTS.lock().unwrap();
        match self.0 {
            "b" => {
                assert_eq!(*events, ["setup-a", "setup-b", "body"]);
                events.push("teardown-b");
            }
            "a" => {
                assert_eq!(*events, ["setup-a", "setup-b", "body", "teardown-b"]);
                events.push("teardown-a");
            }
            _ => unreachable!(),
        }
    }
}

#[aura_integration_test(services = [OrderedService("a"), OrderedService("b")])]
async fn should_set_up_in_order_and_tear_down_in_reverse_order() {
    record("body");
}

struct PanicService;

#[async_trait]
impl IntegrationTestService for PanicService {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        EVENTS.lock().unwrap().clear();
        record("setup");
    }

    async fn tear_down(&self) {
        let mut events = EVENTS.lock().unwrap();
        assert_eq!(*events, ["setup", "body"]);
        events.push("teardown");
    }
}

#[aura_integration_test(services = [PanicService])]
#[should_panic(expected = "test-body panic")]
async fn should_tear_down_and_rethrow_test_body_panic() {
    record("body");
    panic!("test-body panic");
}
