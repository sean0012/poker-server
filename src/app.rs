use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware,
    response::Response,
    routing::get,
};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast};

use crate::{
    log::{log_activity, log_request},
    models::{CreateGame, Game, Role, ServerEvent},
    service::GameService,
    state::{
        AppState, GameUpdates, Games, GuestProfile, GuestProfiles, NEXT_ACTIVITY_ID,
        get_or_create_guest_profile,
    },
};

const MAX_GAME_NAME_LENGTH: usize = 40;
const SESSION_COOKIE_NAME: &str = "poker_session";

pub async fn run() {
    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point to the Supabase PokerDev PostgreSQL database");
    let database = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&database_url)
        .await
        .expect("Failed to connect to the configured PostgreSQL database");
    sqlx::query("SELECT 1")
        .execute(&database)
        .await
        .expect("Could not verify the Supabase PokerDev PostgreSQL connection");

    let games: Games = Arc::new(Mutex::new(HashMap::new()));
    let updates: GameUpdates = Arc::new(Mutex::new(HashMap::new()));
    let guest_profiles: GuestProfiles = Arc::new(Mutex::new(HashMap::new()));
    let state = AppState {
        games,
        game_updates: updates,
        guest_profiles,
        _database: database,
    };

    let app = Router::new()
        .route("/session", axum::routing::post(create_or_restore_session))
        .route("/games", get(list_games).post(create_game))
        .route("/games/{game}", get(get_game_info))
        .route("/games/{game}/player", get(join_as_player))
        .route("/games/{game}/spectator", get(join_as_spectator))
        .layer(middleware::from_fn(log_request))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("포트 바인딩 실패");

    println!("Server listening on port 3000");

    axum::serve(listener, app).await.expect("서버 실행 실패");
}

async fn create_or_restore_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<
    (
        [(axum::http::HeaderName, HeaderValue); 1],
        Json<crate::models::SessionResponse>,
    ),
    ApiError,
> {
    let existing_token = get_cookie(&headers, SESSION_COOKIE_NAME);
    let mut profiles = state.guest_profiles.lock().await;
    let (token, profile) = get_or_create_guest_profile(&mut profiles, existing_token);
    let secure = std::env::var("COOKIE_SECURE")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let secure_attribute = if secure { "; Secure" } else { "" };
    let cookie = HeaderValue::from_str(&format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax{secure_attribute}"
    ))
    .expect("session cookie header must be valid");

    Ok((
        [(axum::http::header::SET_COOKIE, cookie)],
        Json(crate::models::SessionResponse {
            user_id: profile.user_id,
            avatar: profile.avatar,
        }),
    ))
}

fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_string()))
}

async fn session_profile_from_cookie(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<GuestProfile, (StatusCode, &'static str)> {
    let token = get_cookie(headers, SESSION_COOKIE_NAME)
        .ok_or((StatusCode::UNAUTHORIZED, "Guest session required"))?;
    state
        .guest_profiles
        .lock()
        .await
        .get(&token)
        .cloned()
        .ok_or((StatusCode::UNAUTHORIZED, "Guest session expired"))
}

async fn create_game(
    State(state): State<AppState>,
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

    let service = GameService::new(state.games);
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

async fn list_games(State(state): State<AppState>) -> Json<Vec<Game>> {
    let service = GameService::new(state.games);
    Json(service.list_games().await)
}

async fn get_game_info(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Game>, ApiError> {
    let service = GameService::new(state.games);
    let game = service
        .get_game(&id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Game not found"))?;

    Ok(Json(game))
}

async fn join_as_player(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    join(id, state, headers, ws, Role::Player).await
}

async fn join_as_spectator(
    Path(id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    join(id, state, headers, ws, Role::Spectator).await
}

async fn join(
    id: String,
    state: AppState,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    role: Role,
) -> Result<Response, ApiError> {
    let profile = session_profile_from_cookie(&state, &headers).await?;
    let service = GameService::new(state.games.clone());
    if !service.game_exists(&id).await {
        return Err((StatusCode::NOT_FOUND, "Game not found"));
    }

    Ok(ws.on_upgrade(move |socket| handle_connection(socket, id, profile, state, role)))
}

async fn handle_connection(
    mut socket: WebSocket,
    id: String,
    profile: GuestProfile,
    state: AppState,
    role: Role,
) {
    let connection_id = NEXT_ACTIVITY_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let activity = |event: &str, seat: Option<usize>, details: serde_json::Value| {
        log_activity(
            event,
            serde_json::json!({
                "connection_id": connection_id, "game": id, "user_id": profile.user_id, "role": role,
                "seat": seat, "details": details,
            }),
        );
    };
    activity("websocket_connected", None, serde_json::json!({}));

    let mut update_receiver = {
        let mut update_channels = state.game_updates.lock().await;
        update_channels
            .entry(id.clone())
            .or_insert_with(|| broadcast::channel(32).0)
            .subscribe()
    };

    let admission = {
        let service = GameService::new(state.games.clone());
        match role {
            Role::Player => match service
                .join_player(&id, profile.user_id, &profile.avatar)
                .await
            {
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
        game: snapshot.clone(),
    };

    let mut disconnect_reason = "stream_ended";
    match send_event(&mut socket, &event).await {
        Ok(()) => {
            activity(
                "websocket_sent",
                seat,
                serde_json::json!({"type": "joined"}),
            );
            if let Some(channel) = state.game_updates.lock().await.get(&id) {
                let _ = channel.send(());
            }

            loop {
                tokio::select! {
                    incoming = socket.recv() => {
                        match incoming {
                            Some(Ok(message)) => {
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
                            Some(Err(error)) => {
                                activity(
                                    "websocket_error",
                                    seat,
                                    serde_json::json!({"operation": "receive", "error": error.to_string()}),
                                );
                                disconnect_reason = "receive_error";
                                break;
                            }
                            None => break,
                        }
                    }
                    update = update_receiver.recv() => {
                        match update {
                            Ok(()) => {
                                if let Ok(game) = GameService::new(state.games.clone()).get_game(&id).await {
                                    if let Err(error) = send_event(&mut socket, &ServerEvent::GameUpdated { game }).await {
                                        activity(
                                            "websocket_error",
                                            seat,
                                            serde_json::json!({"operation": "send_update", "error": error.to_string()}),
                                        );
                                        disconnect_reason = "send_error";
                                        break;
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                activity(
                                    "websocket_update_lagged",
                                    seat,
                                    serde_json::json!({"skipped": skipped}),
                                );
                                if let Ok(game) = GameService::new(state.games.clone()).get_game(&id).await {
                                    if let Err(error) = send_event(&mut socket, &ServerEvent::GameUpdated { game }).await {
                                        activity(
                                            "websocket_error",
                                            seat,
                                            serde_json::json!({"operation": "send_resync", "error": error.to_string()}),
                                        );
                                        disconnect_reason = "send_error";
                                        break;
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
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

    if GameService::new(state.games.clone())
        .leave_connection(&id, seat)
        .await
        .is_some()
    {
        if let Some(channel) = state.game_updates.lock().await.get(&id) {
            let _ = channel.send(());
        }
    }
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
