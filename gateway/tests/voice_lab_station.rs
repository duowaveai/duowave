//! The voice lab in the station's build: not part of it, and it says so in the envelope. Every build runs this; a
//! computer's has the voice lab (tests/voice_lab.rs) and passes it by.

/// The station's build: the voice lab is not part of it, and says so in the envelope.
mod station {
    use duowave::config::{Config, Env};
    use reqwest::StatusCode;

    #[tokio::test]
    async fn says_the_voice_lab_is_not_part_of_this_build() {
        let app = duowave::app::App::build(Config::for_tests(&Env::from_pairs([("STATE_DIR", "")]))).unwrap();
        // A computer's build has the voice lab (tests/voice_lab.rs).
        if duowave::core::build::computer() {
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(duowave::app::serve(app, listener, std::future::pending()));
        let response = duowave::utils::http::client().get(format!("{base}/api/v1/voice-lab")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["code"], 501);
        assert!(body["message"].as_str().unwrap().contains("нет в этой сборке"), "{body}");
    }
}
