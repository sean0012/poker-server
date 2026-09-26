use std::{collections::HashMap, sync::Arc};

use sqlx::PgPool;
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

use crate::models::Game;

pub type Games = Arc<Mutex<HashMap<String, Game>>>;
pub type GameUpdates = Arc<Mutex<HashMap<String, broadcast::Sender<()>>>>;
pub type GuestProfiles = Arc<Mutex<HashMap<String, GuestProfile>>>;

#[derive(Clone)]
pub struct GuestProfile {
    pub user_id: Uuid,
    pub avatar: String,
}

#[derive(Clone)]
pub struct AppState {
    pub games: Games,
    pub game_updates: GameUpdates,
    pub guest_profiles: GuestProfiles,
    pub _database: PgPool,
}

pub fn get_or_create_guest_profile(
    profiles: &mut HashMap<String, GuestProfile>,
    existing_token: Option<String>,
) -> (String, GuestProfile) {
    if let Some(token) = existing_token {
        if let Some(profile) = profiles.get(&token) {
            return (token, profile.clone());
        }
    }

    let token = Uuid::new_v4().simple().to_string();
    let profile = GuestProfile {
        user_id: Uuid::new_v4(),
        avatar: "puppy".to_string(),
    };
    profiles.insert(token.clone(), profile.clone());
    (token, profile)
}

pub static NEXT_GAME_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
pub static NEXT_ACTIVITY_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(test)]
mod tests {
    use super::{GuestProfile, get_or_create_guest_profile};
    use std::collections::HashMap;

    #[test]
    fn reuses_profile_for_process_session_token() {
        let mut profiles = HashMap::<String, GuestProfile>::new();
        let (token, first) = get_or_create_guest_profile(&mut profiles, None);
        let (same_token, second) = get_or_create_guest_profile(&mut profiles, Some(token.clone()));

        assert_eq!(token, same_token);
        assert_eq!(first.user_id, second.user_id);
        assert_eq!(first.avatar, "puppy");
    }

    #[test]
    fn unknown_cookie_gets_a_new_in_memory_profile() {
        let mut profiles = HashMap::<String, GuestProfile>::new();
        let (old_token, old_profile) = get_or_create_guest_profile(&mut profiles, None);
        profiles.clear();

        let (new_token, new_profile) =
            get_or_create_guest_profile(&mut profiles, Some(old_token.clone()));

        assert_ne!(old_token, new_token);
        assert_ne!(old_profile.user_id, new_profile.user_id);
        assert_eq!(new_profile.avatar, "puppy");
    }
}
