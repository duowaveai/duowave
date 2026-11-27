//! The door between the audio interface and Thinking («Мышление»; docs/plans/audio-thinking-contract.md): one place
//! where a heard phrase goes in and the answer comes out. It holds the messages of both sides, the `Thinking` trait and
//! the spec of a device's hands, and nothing of either side: it depends on none of DuoWave's floors, only on serde and
//! the basics of async, so it stands below all of them (gateway/layers.json, the floor «Дверь») and either side can move
//! to a repository of its own and take the door by its version (`door::V`).

pub mod door;
