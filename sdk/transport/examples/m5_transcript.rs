//! Test-only replayable negotiation fixture for the funded M5/M7 harness.

use erebus_core::auth::Role;
use erebus_transport::{
    hashing::TRANSCRIPT_HASH_VERSION,
    message::{Message, MessageType},
    transcript::Transcript,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let deal: [u8; 16] = hex::decode(std::env::args().nth(1).ok_or("missing deal id")?)?
        .try_into()
        .map_err(|_| "deal id must have 16 bytes")?;
    println!("{}", fixture(deal)?);
    Ok(())
}

fn fixture(deal: [u8; 16]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut transcript = Transcript::new(deal, TRANSCRIPT_HASH_VERSION)?;
    let mut messages = Vec::new();
    for (role, kind, body) in [
        (
            Role::Buyer,
            MessageType::Offer,
            "Offer 60 base units for the test service",
        ),
        (
            Role::Seller,
            MessageType::Counter,
            "Counter 70 base units for the test service",
        ),
    ] {
        let message = Message::new(
            [7; 32],
            deal,
            1,
            role,
            1,
            [0; 32],
            kind,
            body.as_bytes().to_vec(),
        )?;
        transcript.append(&message)?;
        messages.push(hex::encode(message.encode()));
    }
    Ok(serde_json::json!({
        "dealIdHex": hex::encode(deal),
        "transcriptHashVersion": TRANSCRIPT_HASH_VERSION,
        "transcriptRootHex": hex::encode(transcript.root()?),
        "messagesHex": messages,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_messages_replay_to_the_exported_root_and_bind_the_deal() {
        let fixture = fixture([1; 16]).unwrap();
        let messages = fixture["messagesHex"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| Message::decode(&hex::decode(value.as_str().unwrap()).unwrap()).unwrap())
            .collect::<Vec<_>>();
        let replay = Transcript::replay([1; 16], TRANSCRIPT_HASH_VERSION, &messages).unwrap();
        assert_ne!(replay.root().unwrap(), [0; 32]);
        assert_eq!(
            fixture["transcriptRootHex"],
            hex::encode(replay.root().unwrap())
        );
        assert!(Transcript::replay([2; 16], TRANSCRIPT_HASH_VERSION, &messages).is_err());
        let mut changed = messages;
        changed[0].body.push(1);
        let changed = Transcript::replay([1; 16], TRANSCRIPT_HASH_VERSION, &changed).unwrap();
        assert_ne!(
            fixture["transcriptRootHex"],
            hex::encode(changed.root().unwrap())
        );
    }
}
