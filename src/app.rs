use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    middleware,
    response::Response,
    routing::get,
};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{
    log::{log_activity, log_request},
    models::{CreateGame, Game, Role, ServerEvent},
    service::GameService,
    state::{Games, NEXT_ACTIVITY_ID},
};

const MAX_GAME_NAME_LENGTH: usize = 40;
const MAX_GUEST_ID_LENGTH: usize = 100;

#[derive(Deserialize)]
struct GuestQuery {
    guest_id: String,
}

pub async fn run() {
    let games: Games = Arc::new(Mutex::new(HashMap::new()));

    let app = Router::new()
        .route("/games", get(list_games).post(create_game))
        .route("/games/{game}", get(get_game_info))
        .route("/games/{game}/player", get(join_as_player))
        .route("/games/{game}/spectator", get(join_as_spectator))
        .layer(middleware::from_fn(log_request))
        .with_state(games);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("포트 바인딩 실패");

    println!("Server listening on port 3000");

    axum::serve(listener, app).await.expect("서버 실행 실패");
}

async fn create_game(
    State(games): State<Games>,
    Json(input): Json<CreateGame>,
) -> Result<(StatusCode, Json<Game>), ApiError> {
    let name = input.name.trim().to_string();

    if name.is_empty() || name.chars().count() > MAX_GAME_NAME_LENGTH {
        return Err((StatusCode::BAD_REQUEST, "Invalid game name"));
    }

    if input.deck_size == 0 || input.deck_size % 10 != 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Deck size must be a positive multiple of 10",
        ));
    }

    if input.initial_chips.contains(&0) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Initial chips must be greater than zero",
        ));
    }

    let service = GameService::new(games);
    let game = service
        .create_game(input)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"))?;

    log_activity(
        "game_created",
        serde_json::json!({
            "game": game.id, "name": game.name, "deck_size": game.deck_size, "initial_chips": game.players.iter().map(|player| player.initial_chips).collect::<Vec<_>>(),
        }),
    );

    Ok((StatusCode::CREATED, Json(game)))
}

async fn list_games(State(games): State<Games>) -> Json<Vec<Game>> {
    let service = GameService::new(games);
    Json(service.list_games().await)
}

async fn get_game_info(
    Path(id): Path<String>,
    State(games): State<Games>,
) -> Result<Json<Game>, ApiError> {
    let service = GameService::new(games);
    let game = service
        .get_game(&id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Game not found"))?;

    Ok(Json(game))
}

async fn join_as_player(
    Path(id): Path<String>,
    Query(query): Query<GuestQuery>,
    State(games): State<Games>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    join(id, query.guest_id, games, ws, Role::Player).await
}

async fn join_as_spectator(
    Path(id): Path<String>,
    Query(query): Query<GuestQuery>,
    State(games): State<Games>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    join(id, query.guest_id, games, ws, Role::Spectator).await
}

async fn join(
    id: String,
    guest_id: String,
    games: Games,
    ws: WebSocketUpgrade,
    role: Role,
) -> Result<Response, ApiError> {
    if !is_valid_guest_id(&guest_id) {
        return Err((StatusCode::BAD_REQUEST, "Invalid guest ID"));
    }

    let service = GameService::new(games.clone());
    if !service.game_exists(&id).await {
        return Err((StatusCode::NOT_FOUND, "Game not found"));
    }

    Ok(ws.on_upgrade(move |socket| handle_connection(socket, id, guest_id, games, role)))
}

fn is_valid_guest_id(guest_id: &str) -> bool {
    !guest_id.is_empty()
        && guest_id.len() <= MAX_GUEST_ID_LENGTH
        && guest_id.starts_with("guest-")
        && guest_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

async fn handle_connection(
    mut socket: WebSocket,
    id: String,
    guest_id: String,
    games: Games,
    role: Role,
) {
    let connection_id = NEXT_ACTIVITY_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let activity = |event: &str, seat: Option<usize>, details: serde_json::Value| {
        log_activity(
            event,
            serde_json::json!({
                "connection_id": connection_id, "game": id, "guest_id": guest_id, "role": role,
                "seat": seat, "details": details,
            }),
        );
    };
    activity("websocket_connected", None, serde_json::json!({}));
    let admission = {
        let service = GameService::new(games.clone());
        match role {
            Role::Player => match service.join_player(&id, &guest_id).await {
                Ok((seat, game)) => Ok((Some(seat), game)),
                Err(message) => Err(message),
            },
            Role::Spectator => match service.join_spectator(&id).await {
                Ok(game) => Ok((None, game)),
                Err(message) => Err(message),
            },
        }
    };

    let (seat, snapshot) = match admission {
        Ok(value) => value,
        Err(message) => {
            activity(
                "join_rejected",
                None,
                serde_json::json!({"reason": message}),
            );
            let event = ServerEvent::Error { message };
            let _ = send_event(&mut socket, &event).await;
            let _ = socket.send(Message::Close(None)).await;
            activity(
                "websocket_disconnected",
                None,
                serde_json::json!({"reason": "join_rejected"}),
            );
            return;
        }
    };

    activity("game_joined", seat, serde_json::json!({}));
    let event = ServerEvent::Joined {
        role,
        seat,
        game: snapshot,
    };

    let mut disconnect_reason = "stream_ended";
    match send_event(&mut socket, &event).await {
        Ok(()) => {
            activity(
                "websocket_sent",
                seat,
                serde_json::json!({"type": "joined"}),
            );
            while let Some(incoming) = socket.recv().await {
                match incoming {
                    Ok(message) => {
                        let (kind, bytes) = match &message {
                            Message::Text(value) => ("text", value.len()),
                            Message::Binary(value) => ("binary", value.len()),
                            Message::Ping(value) => ("ping", value.len()),
                            Message::Pong(value) => ("pong", value.len()),
                            Message::Close(_) => ("close", 0),
                        };
                        activity(
                            "websocket_received",
                            seat,
                            serde_json::json!({"type": kind, "bytes": bytes}),
                        );
                        if matches!(message, Message::Close(_)) {
                            disconnect_reason = "client_close";
                            break;
                        }
                    }
                    Err(error) => {
                        activity(
                            "websocket_error",
                            seat,
                            serde_json::json!({"operation": "receive", "error": error.to_string()}),
                        );
                        disconnect_reason = "receive_error";
                        break;
                    }
                }
            }
        }
        Err(error) => {
            activity(
                "websocket_error",
                seat,
                serde_json::json!({"operation": "send", "error": error.to_string()}),
            );
            disconnect_reason = "send_error";
        }
    }

    GameService::new(games).leave_connection(&id, seat).await;
    activity("game_left", seat, serde_json::json!({}));
    activity(
        "websocket_disconnected",
        seat,
        serde_json::json!({"reason": disconnect_reason}),
    );
}

async fn send_event(socket: &mut WebSocket, event: &ServerEvent) -> Result<(), axum::Error> {
    let text = serde_json::to_string(event).expect("서버 이벤트 JSON 직렬화 실패");

    socket.send(Message::Text(text.into())).await
}

type ApiError = (StatusCode, &'static str);
