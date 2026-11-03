//! WordWarZ rules engine. Pure: no IO, no clock, no ambient randomness (the
//! game seed drives `rng`). The server applies moves with it; the app
//! previews them with the same code through UniFFI, so both always agree.

pub mod board;
pub mod bot;
pub mod config;
pub mod game;
pub mod letters;
pub mod lexicon;
pub mod movegen;
pub mod moves;
pub mod rng;
pub mod tiles;
pub mod tilesets;
