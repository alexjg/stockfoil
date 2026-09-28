use eyre::{Result, bail};
use stockfoil_mv_register::sync::{self, RemoteServer};
use subduction_core::peer::id::PeerId;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let usage = "Usage: stockfoil-mv-register [--server ws://HOST:PORT --server-peer HEX_PUBLIC_KEY]\n\
                 Without arguments, starts an in-memory loopback Subduction server.";
    let remote = match args.as_slice() {
        [] => None,
        [help] if help == "--help" || help == "-h" => {
            println!("{usage}");
            return Ok(());
        }
        [server, url, peer, public_key] if server == "--server" && peer == "--server-peer" => {
            let mut key = [0; 32];
            hex::decode_to_slice(public_key, &mut key)?;
            Some(RemoteServer {
                url: url.clone(),
                peer_id: PeerId::new(key),
            })
        }
        _ => bail!("{usage}"),
    };
    sync::run(remote).await
}
