package io.qpwgraph.relay

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class RelayJsonTest {

    @Test
    fun a_creation_response_yields_its_handle() {
        val handle = RelayJson.createdHandle(
            JSONObject().put("type", "created").put("handle", 42L).toString(),
        ) { "null handle" }
        assertEquals(42L, handle)
    }

    @Test
    fun a_creation_error_is_raised_with_its_own_message() {
        val error = assertThrows(IllegalStateException::class.java) {
            RelayJson.createdHandle(
                JSONObject().put("type", "error").put("message", "no permission").toString(),
            ) { "null handle" }
        }
        assertEquals("no permission", error.message)
    }

    /** A zero handle is not a handle; accepting it would leak a null pointer. */
    @Test
    fun the_null_handle_is_rejected_with_the_callers_wording() {
        val error = assertThrows(IllegalArgumentException::class.java) {
            RelayJson.createdHandle(
                JSONObject().put("type", "created").put("handle", 0L).toString(),
            ) { "could not create" }
        }
        assertEquals("could not create", error.message)
    }

    @Test
    fun an_unexpected_response_type_is_rejected() {
        assertThrows(IllegalArgumentException::class.java) {
            RelayJson.createdHandle(JSONObject().put("type", "status").toString()) { "x" }
        }
    }

    /** Credentials are seeded to native as id/secret pairs and nothing else. */
    @Test
    fun trusted_credentials_carry_only_the_identity_and_the_secret() {
        val json = JSONArray(
            RelayJson.trustedPeersJson(
                listOf(TrustedRelayPeer("desktop", "ab".repeat(32), "Studio", "10.0.0.2:48123")),
            ),
        )
        val peer = json.getJSONObject(0)

        assertEquals(1, json.length())
        assertEquals("desktop", peer.optString("peer_id"))
        assertEquals("ab".repeat(32), peer.optString("secret"))
        assertEquals(2, peer.length())
    }

    @Test
    fun discovered_peers_round_trip_through_their_wire_shape() {
        val peers = listOf(
            DiscoveredPeer("desktop", "Studio", "10.0.0.2:48123", "wifi"),
            DiscoveredPeer("laptop", "Laptop", "192.168.42.1:48123", "usb"),
        )
        assertEquals(peers, RelayJson.discoveredPeers(RelayJson.discoveredPeersJson(peers)))
    }

    @Test
    fun sessions_are_read_out_of_a_host_status() {
        val status = JSONObject().put(
            "sessions",
            JSONArray().put(
                JSONObject()
                    .put("id", 7L)
                    .put("name", "pixel")
                    .put("address", "192.168.42.2:52000")
                    .put("sending", true)
                    .put("receiving", false)
                    .put("transport", "udp")
                    .put("link", "usb")
                    .put("control_state", "established")
                    .put("audio_channel_state", "streaming")
                    .put("trusted", true),
            ),
        )
        assertEquals(
            listOf(
                RelaySessionInfo(
                    id = 7L,
                    name = "pixel",
                    address = "192.168.42.2:52000",
                    sending = true,
                    receiving = false,
                    transport = "udp",
                    link = "usb",
                    controlState = "established",
                    audioChannelState = "streaming",
                    trusted = true,
                ),
            ),
            RelayJson.sessions(status),
        )
    }

    @Test
    fun a_status_without_sessions_reads_as_none() {
        assertTrue(RelayJson.sessions(JSONObject().put("type", "status")).isEmpty())
    }

    @Test
    fun local_links_are_read_out_of_a_link_snapshot() {
        val raw = JSONObject()
            .put("type", "links")
            .put(
                "links",
                JSONArray().put(
                    JSONObject().put("name", "rndis0").put("addr", "192.168.42.2").put("kind", "usb"),
                ),
            )
            .toString()
        assertEquals(listOf(LocalLinkInfo("rndis0", "192.168.42.2", "usb")), RelayJson.localLinks(raw))
    }

    /** Anything that is not a link snapshot leaves the last state standing. */
    @Test
    fun a_non_link_response_reads_as_no_snapshot() {
        assertNull(RelayJson.localLinks(JSONObject().put("type", "error").toString()))
        assertNull(RelayJson.localLinks("not json at all"))
        assertNull(RelayJson.localLinks(JSONObject().put("type", "links").toString()))
    }

    /**
     * A poll that answers with an object rather than an array is native
     * saying the handle is gone. A healthy empty poll must not look the same.
     */
    @Test
    fun an_invalidated_handle_is_distinguishable_from_an_empty_poll() {
        val failure = RelayJson.pollError(
            JSONObject()
                .put("type", "error")
                .put("message", "client handle is unknown")
                .put("code", "unknown_client_handle")
                .toString(),
        )
        assertEquals("client handle is unknown", failure?.message)
        assertTrue(failure!!.unknownHandle)

        assertNull(RelayJson.pollError("[]"))
        assertNull(RelayJson.pollError(JSONObject().put("type", "status").toString()))
    }

    /** Some errors are recoverable; only the handle code retires the handle. */
    @Test
    fun a_transient_poll_error_does_not_claim_the_handle_is_gone() {
        val failure = RelayJson.pollError(
            JSONObject().put("type", "error").put("message", "busy").put("code", "busy").toString(),
        )
        assertEquals("busy", failure?.message)
        assertEquals(false, failure?.unknownHandle)
    }
}
