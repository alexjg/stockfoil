//! Test control only: JSON carries configuration/commands, never history.
//! History travels through the actual Subduction WebSocket server.
use super::*;
use serde::Deserialize;
use serde_json::{Value, json};
use stockfoil::Author;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Command {
    Configure { authors: Vec<String> },
    Set { key: String, value: String },
    Exchange,
    Get { key: String },
    MakeFragment,
    Fresh,
    Quit,
}

async fn reply(value: Value) -> Result<()> {
    let mut out = tokio::io::stdout();
    out.write_all(format!("{value}\n").as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

pub async fn run() -> Result<()> {
    let mut server = transport::start_server().await?;
    let remote = RemoteServer {
        url: format!("ws://{}", server.address()),
        peer_id: server.peer_id(),
    };
    let mut replica = None;
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        replica = Some(Replica::new(Client::connect(&remote).await?));
        let replica = replica.as_mut().unwrap();
        let id = SedimentreeId::new(rand::random());
        let mut doc = MvRegisterMap::new();
        let mut permissions = HashMap::new();
        reply(json!({
            "serverUrl": remote.url,
            "peerId": hex::encode(remote.peer_id.as_bytes()),
            "documentId": hex::encode(id.as_bytes()),
            "author": hex::encode(doc.author().as_array()),
        })).await?;
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Some(line) = lines.next_line().await? {
            let command: Command = serde_json::from_str(&line)?;
            let response = match command {
                Command::Configure { authors } => {
                    permissions = authors.into_iter().map(|key| {
                        let bytes = hex::decode(key)?;
                        Ok((Author::try_from(bytes.as_slice())?, WritePermission::Unbounded))
                    }).collect::<Result<_>>()?;
                    doc.set_write_permissions(&permissions);
                    json!({"ok": true})
                }
                Command::Set { key, value } => {
                    let hash = doc.set(&key, &value)?;
                    json!({"hash": hex::encode(hash.as_bytes())})
                }
                Command::Exchange => {
                    replica.publish(id, &doc).await?;
                    replica.sync(id).await?;
                    let pending = replica.load(id, &mut doc).await?;
                    json!({"pending": pending, "values": doc.get("title")})
                }
                Command::Get { key } => json!({"values": doc.get(&key)}),
                Command::MakeFragment => {
                    // Stop only once the certified cover really contains a fragment.
                    let mut found = None;
                    for n in 0..8192 {
                        let hash = doc.set("rust-chain", &n.to_string())?;
                        let (cover, _) = doc.graph.export_changes(&ExportCursor::default())?;
                        if cover.added.iter().any(|item| matches!(&item.metadata, TransportMetadata::Fragment { range, .. } if range.head == hash)) {
                            found = Some((n, hash));
                            break;
                        }
                    }
                    ensure!(found.is_some(), "no fragment within bounded test chain");
                    let (last, head) = found.unwrap();
                    json!({"fragment": true, "last": last.to_string(), "head": hex::encode(head.as_bytes())})
                }
                Command::Fresh => {
                    // New graph, signer, transport store, and export cursor.
                    let next = Replica::new(Client::connect(&remote).await?);
                    let old = std::mem::replace(replica, next);
                    old.client.close().await?;
                    doc = MvRegisterMap::new();
                    doc.set_write_permissions(&permissions);
                    json!({"ok": true})
                }
                Command::Quit => {
                    reply(json!({"ok": true})).await?;
                    break;
                }
            };
            reply(response).await?;
        }
        Ok(())
    }).await;
    let mut result = result.map_err(eyre::Report::from).and_then(|result| result);
    if let Some(replica) = replica {
        result = transport::with_cleanup(result, replica.client.close().await);
    }
    let cleanup = tokio::time::timeout(Duration::from_secs(5), server.stop_and_drain())
        .await
        .map_err(eyre::Report::from);
    transport::with_cleanup(result, cleanup)
}
