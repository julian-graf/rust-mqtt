//! Contains utilities for session management.

pub use flight::{ClientPublishState, InFlightPublish, ServerPublishState};
use heapless::Vec;

use crate::types::PacketIdentifier;

mod flight;

/// Session-associated information
///
/// Client identifier is not stored here as it would lead to inconsistencies with the underyling allocation system.
#[derive(Debug, Default, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Session<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize> {
    /// The currently in-flight outgoing publications.
    pub pending_client_publishes: Vec<InFlightPublish<ClientPublishState>, SEND_MAXIMUM>,
    /// The currently in-flight incoming publications.
    pub pending_server_publishes: Vec<InFlightPublish<ServerPublishState>, RECEIVE_MAXIMUM>,
}

impl<const RECEIVE_MAXIMUM: usize, const SEND_MAXIMUM: usize>
    Session<RECEIVE_MAXIMUM, SEND_MAXIMUM>
{
    /// Returns whether the packet identifier is currently in-flight in a client->server publication process.
    #[must_use]
    pub fn is_used_client_publish_packet_identifier(
        &self,
        packet_identifier: PacketIdentifier,
    ) -> bool {
        self.client_publish_state(packet_identifier).is_some()
    }
    /// Returns whether the packet identifier is currently in-flight in a server->client publication process.
    #[must_use]
    pub fn is_used_server_publish_packet_identifier(
        &self,
        packet_identifier: PacketIdentifier,
    ) -> bool {
        self.server_publish_state(packet_identifier).is_some()
    }

    /// Returns the state of the publication of the packet identifier if the packet identifier is in-flight in an outgoing publication.
    #[must_use]
    pub fn client_publish_state(
        &self,
        packet_identifier: PacketIdentifier,
    ) -> Option<ClientPublishState> {
        self.pending_client_publishes
            .iter()
            .find(|f| f.packet_identifier == packet_identifier)
            .map(|f| f.state)
    }
    /// Returns the state of the publication of the packet identifier if the packet identifier is in-flight in an incoming publication.
    #[must_use]
    pub fn server_publish_state(
        &self,
        packet_identifier: PacketIdentifier,
    ) -> Option<ServerPublishState> {
        self.pending_server_publishes
            .iter()
            .find(|f| f.packet_identifier == packet_identifier)
            .map(|f| f.state)
    }

    /// Returns the amount of currently in-flight outgoing publications.
    #[must_use]
    pub fn in_flight_client_publishes(&self) -> u16 {
        self.pending_client_publishes.len() as u16
    }
    /// Returns the amount of currently in-flight incoming publications.
    #[must_use]
    pub fn in_flight_server_publishes(&self) -> u16 {
        self.pending_server_publishes.len() as u16
    }
    /// Returns the amount of slots for outgoing publications.
    #[must_use]
    pub fn client_publish_remaining_capacity(&self) -> u16 {
        (self.pending_client_publishes.capacity() - self.pending_client_publishes.len()) as u16
    }
    /// Returns the amount of slots for incoming publications.
    #[must_use]
    pub fn server_publish_remaining_capacity(&self) -> u16 {
        (self.pending_server_publishes.capacity() - self.pending_server_publishes.len()) as u16
    }

    /// Adds an entry to await or schedule a PUBACK/PUBREC/PUBREL/PUBCOMP packet
    /// for an incoming/server publication. Assumes the packet identifier has no
    /// entry currently.
    ///
    /// # Safety
    /// `self.pending_server_publishes` has free capacity.
    pub(crate) unsafe fn schedule_server(
        &mut self,
        packet_identifier: PacketIdentifier,
        state: ServerPublishState,
    ) {
        unsafe {
            self.pending_server_publishes
                .push_unchecked(InFlightPublish {
                    packet_identifier,
                    state,
                })
        }
    }
    /// Adds an entry to await or schedule a PUBACK/PUBREC/PUBREL/PUBCOMP packet.
    /// for an outgoing/client publication. Assumes the packet identifier has no
    /// entry currently.
    ///
    /// # Safety
    /// `self.pending_client_publishes` has free capacity.
    pub(crate) unsafe fn schedule_client(
        &mut self,
        packet_identifier: PacketIdentifier,
        state: ClientPublishState,
    ) {
        // Safety: self.pending_client_publishes has free capacity.
        unsafe {
            self.pending_client_publishes
                .push(InFlightPublish {
                    packet_identifier,
                    state,
                })
                .unwrap_unchecked();
        }
    }

    pub(crate) fn remove_client_publish(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<ClientPublishState> {
        self.pending_client_publishes
            .iter()
            .position(|s| s.packet_identifier == packet_identifier)
            .map(|i| {
                // Safety: `.iter().position()` confirms the index is within bounds.
                unsafe { self.pending_client_publishes.swap_remove_unchecked(i) }.state
            })
    }
    pub(crate) fn remove_server_publish(
        &mut self,
        packet_identifier: PacketIdentifier,
    ) -> Option<ServerPublishState> {
        self.pending_server_publishes
            .iter()
            .position(|s| s.packet_identifier == packet_identifier)
            .map(|i| {
                // Safety: `.iter().position()` confirms the index is within bounds.
                unsafe { self.pending_server_publishes.swap_remove_unchecked(i) }.state
            })
    }

    pub(crate) fn clear(&mut self) {
        self.pending_client_publishes.clear();
        self.pending_server_publishes.clear();
    }
}
