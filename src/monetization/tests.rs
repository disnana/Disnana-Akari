use std::time::Duration;

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    time::timeout,
};

use super::*;

const APPLICATION: u64 = 999_184_799_365_857_331;
const SKU: u64 = 999_184_799_365_857_333;
const OWNER: u64 = 847_184_799_365_857_999;

async fn local() -> (TcpListener, Client) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::builder()
        .proxy(listener.local_addr().unwrap().to_string(), true)
        .build();
    (listener, http)
}

async fn reply(listener: &TcpListener, owner_type: u8, status: &str, body: Value) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = BufReader::new(socket);
    let mut line = String::new();
    socket.read_line(&mut line).await.unwrap();
    assert_eq!(
        line.trim_end(),
        format!("POST /api/v10/applications/{APPLICATION}/entitlements HTTP/1.1")
    );
    let mut content_length = None;
    loop {
        line.clear();
        assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
        if line.trim().is_empty() {
            break;
        }
        if let Some((header, value)) = line.split_once(':')
            && header.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    let mut request = vec![0; content_length.unwrap()];
    socket.read_exact(&mut request).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&request).unwrap(),
        json!({ "sku_id": SKU.to_string(), "owner_id": OWNER.to_string(), "owner_type": owner_type }),
    );

    let response = body.to_string();
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.len()
    );
    socket
        .get_mut()
        .write_all(headers.as_bytes())
        .await
        .unwrap();
    socket
        .get_mut()
        .write_all(response.as_bytes())
        .await
        .unwrap();
}

fn partial() -> Value {
    json!({
        "id": "1234",
        "sku_id": SKU.to_string(),
        "application_id": APPLICATION.to_string(),
        "type": 4,
        "deleted": false
    })
}

async fn create(
    http: &Client,
    owner: CreateTestEntitlementOwner,
) -> Result<Entitlement, CreateTestEntitlementError> {
    create_test_entitlement(http, Id::new(APPLICATION), Id::new(SKU), owner).await
}

#[tokio::test]
async fn typed_owners_use_string_ids_and_partial_responses_are_supported() {
    let (listener, http) = local().await;
    let server = tokio::spawn(async move {
        reply(&listener, 1, "200 OK", partial()).await;
        let mut user = partial();
        user["user_id"] = json!(OWNER.to_string());
        reply(&listener, 2, "200 OK", user).await;
    });
    timeout(Duration::from_secs(5), async {
        let guild = create(&http, CreateTestEntitlementOwner::Guild(Id::new(OWNER)))
            .await
            .unwrap();
        assert_eq!(guild.id, Id::new(1234));
        assert!(guild.guild_id.is_none());
        assert!(guild.starts_at.is_none());
        assert!(guild.ends_at.is_none());
        let user = create(&http, CreateTestEntitlementOwner::User(Id::new(OWNER)))
            .await
            .unwrap();
        assert_eq!(user.user_id, Some(Id::new(OWNER)));
        server.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn mismatched_response_and_http_or_decode_errors_remain_distinct() {
    let (listener, http) = local().await;
    let server = tokio::spawn(async move {
        for key in ["application_id", "sku_id", "guild_id"] {
            let mut body = partial();
            body[key] = json!("2345");
            reply(&listener, 1, "200 OK", body).await;
        }
        let mut body = partial();
        body["user_id"] = json!("2345");
        reply(&listener, 2, "200 OK", body).await;
        reply(
            &listener,
            1,
            "403 Forbidden",
            json!({"code": 50001, "message": "Missing Access"}),
        )
        .await;
        reply(&listener, 1, "200 OK", json!({})).await;
    });
    timeout(Duration::from_secs(5), async {
        for _ in 0..3 {
            assert!(matches!(
                create(&http, CreateTestEntitlementOwner::Guild(Id::new(OWNER))).await,
                Err(CreateTestEntitlementError::MismatchedResponse)
            ));
        }
        assert!(matches!(
            create(&http, CreateTestEntitlementOwner::User(Id::new(OWNER))).await,
            Err(CreateTestEntitlementError::MismatchedResponse)
        ));
        assert!(matches!(
            create(&http, CreateTestEntitlementOwner::Guild(Id::new(OWNER))).await,
            Err(CreateTestEntitlementError::Http(_))
        ));
        assert!(matches!(
            create(&http, CreateTestEntitlementOwner::Guild(Id::new(OWNER))).await,
            Err(CreateTestEntitlementError::HttpBody(_))
        ));
        server.await.unwrap();
    })
    .await
    .unwrap();
}
