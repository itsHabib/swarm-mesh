use anyhow::{Context, Result};
use snow::{Builder, HandshakeState, params::NoiseParams};

/// Initiates a Noise protocol handshake as the initiator.
///
/// This function creates a new Noise handshake state in initiator mode and
/// generates the first handshake message. The initiator is responsible for
/// starting the handshake process by sending the first message (-> e pattern).
///
/// # Arguments
/// * `static_keys` - The node's long-term Noise keypair for authentication
/// * `network_key` - Pre-shared key for network-level authentication
///
/// # Returns
/// A tuple containing:
/// * `HandshakeState` - The ongoing handshake state for subsequent messages
/// * `Vec<u8>` - The first handshake message to send to the responder
///
/// # Errors
/// Returns an error if the handshake state creation or message generation fails.
///
/// # Security
/// The function uses the XXpsk3 pattern with a pre-shared key in slot 3,
/// providing mutual authentication and forward secrecy.
pub fn start_handshake(
    static_keys: &snow::Keypair,
    network_key: &[u8; 32],
) -> Result<(HandshakeState, Vec<u8>)> {
    let noise_params = noise_params().unwrap();
    let mut initiator_state = Builder::new(noise_params)
        .local_private_key(&static_keys.private)
        .psk(3, network_key)
        .build_initiator()?;

    let mut buf = [0u8; 1024];
    // -> e
    let len = initiator_state.write_message(&[], &mut buf)?;
    let first_message = buf[..len].to_vec();

    Ok((initiator_state, first_message))
}

/// Creates a Noise protocol handshake state in responder mode.
///
/// This function builds a responder handshake state that can process
/// incoming handshake messages from an initiator. The responder waits
/// for the initiator to send the first message before participating
/// in the handshake.
///
/// # Arguments
/// * `static_keys` - The node's long-term Noise keypair for authentication
/// * `network_key` - Pre-shared key for network-level authentication
///
/// # Returns
/// A `HandshakeState` configured as a responder, ready to process
/// the first handshake message from an initiator.
///
/// # Errors
/// Returns an error if the responder state creation fails.
///
/// # Security
/// Uses the same XXpsk3 pattern as the initiator, ensuring compatible
/// cryptographic parameters and mutual authentication.
pub fn build_responder(static_keys: &snow::Keypair, network_key: &[u8]) -> Result<HandshakeState> {
    Builder::new(noise_params()?)
        .local_private_key(&static_keys.private)
        .psk(3, &network_key)
        .build_responder()
        .context("Failed to build responder state")
}

/// Returns the Noise protocol parameters used by the mesh network.
///
/// This function defines the specific Noise protocol configuration used
/// for all secure communications in the mesh network. The chosen parameters
/// provide strong security guarantees and good performance characteristics.
///
/// # Returns
/// `NoiseParams` configured with:
/// * **Pattern**: XX (mutual authentication with static keys)
/// * **PSK**: psk3 (pre-shared key in slot 3 for network authentication)
/// * **DH**: 25519 (Curve25519 elliptic curve Diffie-Hellman)
/// * **Cipher**: ChaChaPoly (ChaCha20-Poly1305 AEAD cipher)
/// * **Hash**: BLAKE2s (fast cryptographic hash function)
///
/// # Errors
/// Returns an error if the parameter string cannot be parsed.
///
/// # Security Properties
/// * **Mutual Authentication**: Both parties prove their identity
/// * **Forward Secrecy**: Past communications remain secure if keys are compromised
/// * **Replay Protection**: Built-in nonce handling prevents replay attacks
/// * **Network Authentication**: PSK ensures only authorized nodes can join
pub fn noise_params() -> Result<NoiseParams> {
    "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s"
        .parse::<NoiseParams>()
        .context("Invalid noise params")
}
