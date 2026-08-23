pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("aura-historia-smoking-test")
        .build()
        .unwrap()
}
