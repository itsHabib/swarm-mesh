use anyhow::{Context, Result};
use snow::{Builder, HandshakeState, TransportState, params::NoiseParams};

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

pub fn build_responder(static_keys: &snow::Keypair, network_key: &[u8]) -> Result<HandshakeState> {
    Builder::new(noise_params()?)
        .local_private_key(&static_keys.private)
        .psk(3, &network_key)
        .build_responder()
        .context("Failed to build responder state")
}

fn noise_params() -> Result<NoiseParams> {
    "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s"
        .parse::<NoiseParams>()
        .context("Invalid noise params")
}
