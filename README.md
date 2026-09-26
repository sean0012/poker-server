# Poker Server

## Local Supabase PokerDev

The server reads `DATABASE_URL` from `.env` and uses SQLx to establish a connection to the Supabase local PostgreSQL project `PokerDev`. It only checks connectivity at startup; it does not run migrations or write guest profiles, sessions, or game state to PostgreSQL.

Start the Supabase DB and get connection string.

Copy `.env.example` to `.env` and replace the placeholder password using the Supabase connection string. Do not commit `.env` or share its contents. Set `COOKIE_SECURE=false` for local HTTP development. In production behind HTTPS, set `COOKIE_SECURE=true`.

Run the server from this directory so dotenv loads `.env`:

```sh
cargo run
```

Guest UUIDs, guest avatar profiles, and session tokens are held only in poker-server process memory. Restarting the server clears them; the next client request creates a new guest profile. Supabase production/local PostgreSQL is not used for profile persistence until OAuth/account persistence is implemented.
