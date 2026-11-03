use std::path::Path;

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{WorldState, shared},
};
use mock_supabase::Supabase;
use tower::ServiceExt;

#[tokio::test]
async fn feedback_submit_then_delete_like_postgrest() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scenarios/example/github/basic");
    let world = shared(WorldState::from_dataset(Dataset::load(&dir).unwrap()).unwrap());
    let app = service_app(world.clone(), &Supabase);
    let submit = Request::post("/rest/v1/rpc/submit_interfaces_feedback")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"feedback":"great","project_ref":"abc"}"#))
        .unwrap();
    let resp = app.clone().oneshot(submit).await.unwrap();
    let token: String =
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(token.len(), 32);
    assert_eq!(
        world.lock().unwrap().snapshot()["services"]["supabase"]["feedback"][&token]["feedback"],
        "great"
    );

    let delete = |t: &str| {
        Request::delete(format!("/rest/v1/interfaces_feedback?delete_token=eq.{t}"))
            .body(Body::empty())
            .unwrap()
    };
    let first = app.clone().oneshot(delete(&token)).await.unwrap();
    assert_eq!(first.status(), 204);
    assert_eq!(first.headers()["content-range"], "*/1");
    let again = app.oneshot(delete(&token)).await.unwrap();
    assert_eq!(again.headers()["content-range"], "*/0");
}
