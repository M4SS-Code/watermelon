use std::{
    hash::{DefaultHasher, Hasher as _},
    time::Duration,
};

use bytes::Bytes;
use futures_util::TryStreamExt as _;
use jiff::Zoned;
use tokio::{
    signal::ctrl_c,
    task::JoinSet,
    time::{MissedTickBehavior, interval},
};
use watermelon::{
    core::Client,
    proto::{
        Subject,
        headers::{HeaderName, HeaderValue},
    },
};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let client = Client::builder()
        .connect("nats://demo.nats.io".parse()?)
        .await?;
    println!("Quick Info: {:?}", client.quick_info());

    let mut set = JoinSet::new();

    // Subscribe to `watermelon.>`, print every message we get and reply if possible
    set.spawn({
        let client = client.clone();

        async move {
            let mut subscription = client
                .subscribe(Subject::from_static("watermelon.>"), None)
                .await?;
            while let Some(msg) = subscription.try_next().await? {
                println!(
                    "Received new message subject={:?} headers={:?} payload={:?}",
                    msg.base.subject, msg.base.headers, msg.base.payload
                );

                if let Some(reply_subject) = msg.base.reply_subject {
                    client
                        .publish(reply_subject)
                        .header(HeaderName::from_static("Local-Time"), local_time())
                        .payload(Bytes::from_static("Welcome from Watermelon!".as_bytes()))
                        .await?;
                }
            }

            Ok::<_, BoxError>(())
        }
    });

    // Publish to `watermelon.[random number]` every 20 seconds and await the response
    set.spawn({
        let client = client.clone();

        async move {
            let mut interval = interval(Duration::from_secs(20));
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

            loop {
                interval.tick().await;

                let subject = format!("watermelon.{}", rng()).try_into()?;
                println!("Sending new request...");
                let response_fut = client
                    .request(subject)
                    .header(HeaderName::from_static("Local-Time"), local_time())
                    .payload(Bytes::from_static(b"Hello from Watermelon!"))
                    .await?;
                println!("Awaiting response...");
                match response_fut.await {
                    Ok(resp) => {
                        println!(
                            "Received response subject={:?} headers={:?} payload={:?}",
                            resp.base.subject, resp.base.headers, resp.base.payload
                        );
                    }
                    Err(err) => {
                        eprintln!("Received error err={err:?}");
                    }
                }
            }
        }
    });

    // Wait for the user to CTRL+C the program and gracefully shutdown the client
    set.spawn(async move {
        ctrl_c().await?;
        println!("Starting graceful shutdown...");
        client.close().await;
        Ok::<_, BoxError>(())
    });

    while let Some(next) = set.join_next().await {
        println!("Task exited with: {next:?}");
    }

    Ok(())
}

/// Get the local time and timezone
fn local_time() -> HeaderValue {
    Zoned::now()
        .to_string()
        .try_into()
        .expect("local DateTime can always be encoded into `HeaderValue`")
}

/// A poor man's RNG
fn rng() -> u64 {
    DefaultHasher::new().finish()
}
