use super::support::Result;
use axum::Router;
use linalab_auth::access_identity::{AccessError, JwksSource};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, process::Command, time::Duration};
use time::OffsetDateTime;

#[derive(Deserialize)]
pub(super) struct Signed {
    pub jwks: Value,
    pub tokens: BTreeMap<String, String>,
}
pub(super) struct Wire(pub String);
#[async_trait::async_trait]
impl JwksSource for Wire {
    async fn fetch(&self) -> std::result::Result<Vec<u8>, AccessError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .map_err(|_| AccessError::Unavailable)?;
        client
            .get(&self.0)
            .send()
            .await
            .map_err(|_| AccessError::Unavailable)?
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|_| AccessError::Unavailable)
    }
}
pub(super) struct Surface {
    pub origin: String,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<std::result::Result<(), std::io::Error>>,
}
impl Surface {
    #[cfg(test)]
    pub async fn start(router: Router) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        // Subscribe to shutdown before triggering the listener's async server task.
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _shutdown = stopped.await;
                })
                .await
        });
        Ok(Self {
            origin,
            stop,
            server,
        })
    }
    pub async fn stop(self) -> Result {
        self.stop.send(()).map_err(|()| "shutdown receiver")?;
        tokio::time::timeout(Duration::from_secs(5), self.server).await???;
        Ok(())
    }
}
pub(super) fn sign() -> Result<Signed> {
    let script = r"
    import {generateKeyPairSync,sign} from 'node:crypto';
    const pair=generateKeyPairSync('rsa',{modulusLength:2048}), now=Number(process.argv[1]);
    const jwt=(sub,email,iat=now-900)=>{
      const h=Buffer.from(JSON.stringify({alg:'RS256',kid:'qa',typ:'JWT'})).toString('base64url');
      const p=Buffer.from(JSON.stringify({iss:'https://linalab.cloudflareaccess.com',aud:['action-qa'],sub,email,type:'app',iat,nbf:iat,exp:now+3600})).toString('base64url');
      return h+'.'+p+'.'+sign('RSA-SHA256',Buffer.from(h+'.'+p),pair.privateKey).toString('base64url');
    };
    process.stdout.write(JSON.stringify({jwks:{keys:[{...pair.publicKey.export({format:'jwk'}),kid:'qa',alg:'RS256',use:'sig'}]},tokens:{admin:jwt('admin','islee@linalab.io'),owner:jwt('owner','owner@example.com'),other:jwt('other','other@example.com'),session:jwt('admin','islee@linalab.io',now-800)}}));
    ";
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", script])
        .arg(OffsetDateTime::now_utc().unix_timestamp().to_string())
        .output()?;
    if !output.status.success() {
        return Err("RSA signer failed".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}
