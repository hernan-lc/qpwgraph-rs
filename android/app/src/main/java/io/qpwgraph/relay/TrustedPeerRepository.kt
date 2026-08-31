package io.qpwgraph.relay

import org.json.JSONObject

/**
 * Trusted credentials, read and written through the encrypted
 * [TrustedCredentialStore].
 *
 * Every operation returns its outcome instead of writing an error into the UI
 * state, so the ViewModel stays the only thing that mutates state and this
 * class stays unit-testable without an Android runtime.
 */
internal class TrustedPeerRepository(private val store: TrustedCredentialStore) {

    /** The stored peers, or [Result.failure] when the record cannot be read. */
    fun peers(): Result<List<TrustedRelayPeer>> = runCatching { store.read() }

    /** Reading failures degrade to "no peers" for callers that only browse. */
    fun peersOrEmpty(): List<TrustedRelayPeer> = peers().getOrDefault(emptyList())

    fun peer(peerId: String): TrustedRelayPeer? =
        peersOrEmpty().firstOrNull { it.peerId == peerId }

    fun summaries(): List<TrustedRelayPeerSummary> =
        peersOrEmpty().map { TrustedRelayPeerSummary(it.peerId, it.name, it.address) }

    fun credentialsJson(): String = RelayJson.trustedPeersJson(peersOrEmpty())

    /**
     * Merge a credential into the record, keeping any name and address the
     * caller did not supply.
     *
     * [TrustedCredentialStore] uses a fresh AES-GCM nonce for every complete
     * record write and commits the encrypted record before any legacy
     * plaintext can be replaced.
     *
     * A blank peer id or secret is not an error: it is a native event that
     * carried nothing worth storing, and it reports [Saved.Skipped].
     */
    fun save(
        peerId: String,
        secret: String,
        name: String = "",
        address: String = "",
    ): Saved {
        if (peerId.isBlank() || secret.isBlank()) return Saved.Skipped
        val previous = peer(peerId)
        val merged = TrustedRelayPeer(
            peerId = peerId,
            secret = secret,
            name = name.ifBlank { previous?.name.orEmpty() },
            address = address.ifBlank { previous?.address.orEmpty() },
        )
        return try {
            store.save(merged)
            Saved.Stored
        } catch (error: Exception) {
            Saved.Failed(error.message.orEmpty())
        }
    }

    /** Re-store a credential exactly as it was, to undo a failed rotation. */
    fun restore(peer: TrustedRelayPeer): Saved =
        save(peer.peerId, peer.secret, peer.name, peer.address)

    fun remove(peerId: String): Result<Unit> = runCatching { store.remove(peerId) }

    /** Pull a credential out of a native event or handshake response. */
    fun saveFrom(event: JSONObject): Saved = save(
        peerId = event.optString("peer_id").ifBlank { event.optString("id") },
        secret = event.optString("secret"),
        name = event.optString("name").ifBlank { event.optString("host") },
        address = event.optString("address"),
    )

    sealed interface Saved {
        /** Stored, and the caller should refresh the summaries it shows. */
        data object Stored : Saved

        /** Nothing to store; not a failure. */
        data object Skipped : Saved

        data class Failed(val message: String) : Saved

        val stored: Boolean get() = this is Stored
    }
}
