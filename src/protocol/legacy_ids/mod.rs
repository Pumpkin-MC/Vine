//! Packet ids of every supported Java version.
//!
//! Adapted from `src/legacy/ids.rs` of pumpkin-java-multiversion
//! (<https://github.com/Pumpkin-MC/pumpkin-java-multiversion>, commit `f750eeba`), with
//! `generated_ids.rs` copied unchanged. Both are used under the MIT licence; see `LICENSE-MIT`
//! in this directory. Changes: the column check uses `JavaMinecraftVersion` directly instead of
//! the plugin's `KNOWN_VERSIONS`, and `serverbound_id` (the inverse of `serverbound_to_current`)
//! was added.

use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_protocol::ConnectionState;
use pumpkin_util::version::JavaMinecraftVersion;

#[path = "generated_ids.rs"]
#[allow(dead_code)]
mod generated;

#[allow(unused_imports)]
pub use generated::{PacketId, VERSIONS, clientbound, serverbound};

// The generated columns must follow `JavaMinecraftVersion` exactly: one column per variant,
// oldest first, the last one being the current version. If Pumpkin adds a version, this fails
// to compile until the table is regenerated.
const _: () = {
    assert!(JavaMinecraftVersion::V_1_7_2 as usize == 0);
    assert!(JavaMinecraftVersion::Unknown as usize == VERSIONS);
    assert!(CURRENT_MC_VERSION as usize == VERSIONS - 1);
};

impl PacketId {
    /// The id in `version`, or `-1` when that version doesn't have the packet. Unknown
    /// versions get the current id.
    #[must_use]
    pub const fn to_id(&self, version: JavaMinecraftVersion) -> i32 {
        let column = version as usize;
        if column < VERSIONS {
            self.0[column] as i32
        } else {
            self.current()
        }
    }

    /// The id in the current version.
    #[must_use]
    pub const fn current(&self) -> i32 {
        self.0[VERSIONS - 1] as i32
    }
}

const fn state_index(state: ConnectionState) -> usize {
    match state {
        ConnectionState::HandShake => 0,
        ConnectionState::Status => 1,
        ConnectionState::Login | ConnectionState::Transfer => 2,
        ConnectionState::Config => 3,
        ConnectionState::Play => 4,
    }
}

fn lookup(
    tables: &[[&[i16]; VERSIONS]; 5],
    state: ConnectionState,
    id: i32,
    version: JavaMinecraftVersion,
) -> Option<i32> {
    let table = tables[state_index(state)].get(version as usize)?;
    let id = *table.get(usize::try_from(id).ok()?)?;
    (id != -1).then_some(i32::from(id))
}

/// The current id of a packet the client sent as `client_id`.
#[must_use]
pub fn serverbound_to_current(
    state: ConnectionState,
    client_id: i32,
    version: JavaMinecraftVersion,
) -> Option<i32> {
    lookup(
        &generated::SERVERBOUND_TO_CURRENT,
        state,
        client_id,
        version,
    )
}

/// The id `version` uses for the current serverbound packet `current_id`; `None` when that
/// version has no such packet.
#[must_use]
pub fn serverbound_id(
    state: ConnectionState,
    current_id: i32,
    version: JavaMinecraftVersion,
) -> Option<i32> {
    let current = i16::try_from(current_id).ok()?;
    let table = generated::SERVERBOUND_TO_CURRENT[state_index(state)].get(version as usize)?;
    let id = table.iter().position(|&id| id == current)?;
    i32::try_from(id).ok()
}

/// The client's id of the current packet `current_id`; `None` when the client has no such packet.
#[must_use]
pub fn clientbound_id(
    state: ConnectionState,
    current_id: i32,
    version: JavaMinecraftVersion,
) -> Option<i32> {
    lookup(
        &generated::CLIENTBOUND_FROM_CURRENT,
        state,
        current_id,
        version,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_maps_to_itself() {
        let id = serverbound::play::CHAT.current();
        assert_eq!(
            serverbound_to_current(ConnectionState::Play, id, CURRENT_MC_VERSION),
            Some(id)
        );
        assert_eq!(
            clientbound_id(ConnectionState::Play, id, CURRENT_MC_VERSION),
            Some(id)
        );
    }

    #[test]
    fn renamed_packets_use_the_older_row() {
        let version = JavaMinecraftVersion::V_1_21_11;
        let swing = serverbound::play::SWING.to_id(version);
        assert_eq!(
            serverbound_to_current(ConnectionState::Play, swing, version),
            Some(serverbound::play::PUNCH.current())
        );
    }

    #[test]
    fn missing_packets_have_no_client_id() {
        let version = JavaMinecraftVersion::V_1_8;
        let bundle = clientbound::play::BUNDLE_DELIMITER.current();
        assert_eq!(clientbound_id(ConnectionState::Play, bundle, version), None);
    }

    #[test]
    fn serverbound_id_inverts_serverbound_to_current() {
        let version = JavaMinecraftVersion::V_1_12_2;
        let current = serverbound::play::CHAT.current();
        let client = serverbound_id(ConnectionState::Play, current, version).unwrap();
        assert_eq!(
            serverbound_to_current(ConnectionState::Play, client, version),
            Some(current)
        );
    }
}
