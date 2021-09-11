use bytes::Bytes;
use prettydiff::{basic::DiffOp, diff_lines};
use reqwest::{Client, StatusCode};
use std::time::Duration;

static NZZ_HREF: &'static str = "https://nzz.ch/";
static NAU_HREF: &'static str = "https://www.nau.ch/";
static ZWM_HREF: &'static str = "https://20min.ch";
static ADM_HREF: &'static str = "https://www.admin.ch/gov/de/start/dokumentation/medienmitteilungen.html?dyn_startDate=01.01.2020&dyn_organization=1";

static SITES: &'static [(&'static str, &'static str)] = &[
    ("Neue Züricher Zeitung", NZZ_HREF),
    ("NAU", NAU_HREF),
    ("20 Minuten", ZWM_HREF),
    ("Admin.ch News", ADM_HREF),
];

#[derive(Debug)]
enum SiteMessage {
    Check,
}

#[derive(Debug)]
struct SiteState {
    name: String,
    href: String,
    client: Client,
    result: Option<SiteResult>,
}

#[derive(Debug)]
struct SiteResult {
    status: StatusCode,
    bytes: Bytes,
}

struct SiteResultDiff {
    status: Option<StatusCode>,
    diff: Option<String>,
}

impl SiteState {
    async fn check(&mut self) -> anyhow::Result<()> {
        log::info!("Checking {}", self.name);
        let response = self
            .client
            .get(&self.href)
            .header("Accept", "text/html")
            .send()
            .await?;

        let status = response.status();
        let bytes = response.bytes().await?;

        let new_result = SiteResult { status, bytes };

        let prev = self.result.take();

        let diff = prev.as_ref().map(|result| result.diff(&new_result));

        if let Some(diff) = diff {
            if diff.is_different() {
                let title = format!("{} Updated", self.name);
                let description = if let Some(status) = diff.status {
                    if let Some(diff) = diff.diff {
                        format!("New status '{}' and site content changed\n{}", status, diff,)
                    } else {
                        format!("New status '{}'", status)
                    }
                } else if let Some(diff) = diff.diff {
                    format!("Site content changed\n{}", diff)
                } else {
                    format!("Site content changed")
                };

                log::info!("{}", title);
                log::info!("{}", description);
                tokio::process::Command::new("notify-send")
                    .args(&["-i", "appointment", &title])
                    .spawn()?
                    .wait()
                    .await?;
            }
        } else {
            log::info!(
                "First check for {}, status: {}",
                self.name,
                new_result.status
            );
        }

        self.result = Some(new_result);
        Ok(())
    }

    async fn handle_message(&mut self, message: SiteMessage) -> anyhow::Result<()> {
        match message {
            SiteMessage::Check => {
                self.check().await?;
            }
        }

        Ok(())
    }
}

impl SiteResult {
    fn diff(&self, rhs: &SiteResult) -> SiteResultDiff {
        let status = if self.status != rhs.status {
            Some(rhs.status.clone())
        } else {
            None
        };

        let old = String::from_utf8_lossy(&self.bytes);
        let new = String::from_utf8_lossy(&rhs.bytes);

        let changeset = diff_lines(&old, &new);

        let diff: Vec<DiffOp<'_, &str>> = changeset
            .diff()
            .into_iter()
            .filter(|op| match op {
                DiffOp::Equal(_) => false,
                _ => true,
            })
            .collect();

        let diff = if !diff.is_empty() {
            Some(render_diff(&diff))
        } else {
            None
        };

        SiteResultDiff { status, diff }
    }
}

impl SiteResultDiff {
    fn is_different(&self) -> bool {
        self.status.is_some() || self.diff.is_some()
    }
}

fn render_diff(ops: &[DiffOp<'_, &str>]) -> String {
    ops.iter().fold(String::new(), |acc, op| match op {
        DiffOp::Equal(_) => acc,
        DiffOp::Insert(a) => acc + "Added:\n" + &a.join("\n") + "\n\n",
        DiffOp::Remove(a) => acc + "Removed:\n" + &a.join("\n") + "\n\n",
        DiffOp::Replace(a, b) => {
            acc + "Replaced:\n" + &a.join("\n") + "\nwith\n" + &b.join("\n") + "\n\n"
        }
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let client = Client::builder().user_agent("Site Checker").build()?;

    let mut root_handle = tokio_actors::root();

    for (name, href) in SITES {
        let name = name.to_string();
        let href = href.to_string();
        let client = client.clone();

        let state = SiteState {
            name,
            href,
            client,
            result: None,
        };

        let handle = root_handle
            .spawn_child(state, move |state, msg, _| {
                Box::pin(async move { state.handle_message(msg).await })
            })
            .await?;
        handle.every(Duration::from_secs(30 * 60), || SiteMessage::Check);
    }

    tokio::signal::ctrl_c().await?;

    root_handle.close().await;

    Ok(())
}
