//! ForeMan entry point: boot as HR, route tasks, run them with mock tools
//! (dry-run), and serve the API.

use std::path::PathBuf;

use anyhow::Context;
use axom_core::authz::AuthorizationKey;
use axom_core::domain::Plan;
use axom_hr::notify::NotifyConfig;
use axom_hr::performance::PerformanceConfig;
use axom_hr::{cap, PolicyConfig};
use axom_tools::price::{HttpPriceFeed, PriceFeed};
use axom_planner::{store, PlannerConfig, RuleBasedPlanner};
use axom_tools::mock::MockTools;
use std::str::FromStr;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

mod testnet_fund;

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `axom keygen`: print a new authorization key pair (attestation or approval).
    if std::env::args().nth(1).as_deref() == Some("keygen") {
        let k = AuthorizationKey::generate();
        println!("# Signing seed: give only to the service that signs (keep secret).");
        println!("SIGNING_KEY={}", k.seed_hex());
        println!("# Verify key: give to axom-signer.");
        println!("VERIFY_KEY={}", k.public_key().to_near_string());
        return Ok(());
    }
    // `axom secrets-key`: print a new master key for AXOM_SECRETS_KEY.
    if std::env::args().nth(1).as_deref() == Some("secrets-key") {
        println!("AXOM_SECRETS_KEY={}", axom_platform::crypto::SecretBox::generate_hex());
        return Ok(());
    }
    // `axom import-company`: bring an existing single-company database onto
    // the multi-tenant platform (DATABASE_URL = the platform database).
    if std::env::args().nth(1).as_deref() == Some("import-company") {
        return axom_app::multi::import_company(std::env::args().skip(2).collect()).await;
    }
    if std::env::args().nth(1).as_deref() == Some("demo") {
        return demo_main().await;
    }
    // `axom testnet-fund [NEAR] [--to ACCOUNT]`: put testnet NEAR in the treasury.
    if std::env::args().nth(1).as_deref() == Some("testnet-fund") {
        return testnet_fund::run(std::env::args().skip(2).collect()).await;
    }
    // `axom intents-tokens [chain]`: assets NEAR Intents can deliver (1Click, mainnet).
    if std::env::args().nth(1).as_deref() == Some("intents-tokens") {
        return intents_tokens(std::env::args().nth(2)).await;
    }
    // `axom intents-quote <asset_id> <address> <usd> [usd_per_near]`: a real
    // NEAR Intents dry-run quote (mainnet, `dry: true`, no funds move) for
    // paying `usd` from the treasury's wNEAR to `address`.
    if std::env::args().nth(1).as_deref() == Some("intents-quote") {
        let a: Vec<String> = std::env::args().skip(2).collect();
        anyhow::ensure!(a.len() >= 3, "usage: axom intents-quote <asset_id> <address> <usd> [usd_per_near]");
        let usd: rust_decimal::Decimal = a[2].parse().context("usd")?;
        let price = match a.get(3) {
            Some(p) => p.parse().context("usd_per_near")?,
            None => live_near_price().await,
        };
        let q = intents_dry_quote(&a[0], &a[1], usd, price).await?;
        println!("{}", serde_json::to_string_pretty(&q)?);
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    // Live mode: configured integrations make real calls (emails, contracts,
    // posts, outreach, tool calls). Dry-run (the default) only records them.
    if env_or("AXOM_LIVE", "false") == "true" {
        tracing::warn!("LIVE MODE: configured integrations make real calls (emails, contracts, posts, outreach, tool calls)");
    } else {
        tracing::info!("dry-run mode: integrations record what they would send without calling the service (AXOM_LIVE=true to go live)");
    }
    if env_or("AXOM_MULTI_TENANT", "false") == "true" {
        return axom_app::multi::run().await;
    }

    let policy = PolicyConfig::load(&PathBuf::from(env_or("AXOM_POLICY_PATH", "config/policy.toml"))).context("loading policy")?;
    let planner_cfg = PlannerConfig::load(&PathBuf::from(env_or("AXOM_PLANNER_CONFIG_PATH", "config/planner.toml"))).context("loading planner config")?;
    let plan_path = PathBuf::from(env_or("AXOM_MASTER_PLAN_PATH", "config/master_plan.json"));
    let plan: Plan = serde_json::from_str(&std::fs::read_to_string(&plan_path).with_context(|| format!("reading {}", plan_path.display()))?)
        .context("parsing master plan")?;

    // DATABASE_URL holds credentials: never log it.
    let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
    // Pin search_path: with Postgres's default "$user", public a role named
    // `axom` would resolve unqualified names (sqlx's _sqlx_migrations) into the
    // axom schema once it exists, and migrations would re-run.
    let opts = PgConnectOptions::from_str(&db_url)
        .context("parsing DATABASE_URL")?
        .options([("search_path", "public")]);
    let pool = PgPoolOptions::new().max_connections(10).connect_with(opts).await.context("connecting to Postgres")?;
    axom_core::MIGRATOR.run(&pool).await.context("running migrations")?;

    let monthly_cap = cap::seed_cap(&pool, &policy).await?;
    tracing::info!(policy_version = policy.version(), %monthly_cap, "policy loaded");

    let planner = RuleBasedPlanner::new(planner_cfg.clone());
    let boot = store::bootstrap(&pool, &policy, &planner_cfg, &planner, &plan).await?;
    tracing::info!(?boot, "booted as HR");

    // Reviews, firing, and owner notification delivery run every 15 seconds,
    // so a notification is emailed well within one minute of its event.
    let perf = PerformanceConfig::load(&PathBuf::from(env_or("AXOM_PERFORMANCE_CONFIG_PATH", "config/performance.toml")))
        .context("loading performance config")?;
    let notify_cfg = NotifyConfig::from_env();
    if notify_cfg.owner_email.is_empty() {
        tracing::warn!("AXOM_OWNER_EMAIL is not set: owner notifications stay in the dashboard feed only");
    }
    // Payroll needs the policy engine's attestation key; without it,
    // payroll does not run (no payout could be sent anyway).
    let payroll_off = env_or("AXOM_DISABLE_PAYROLL", "false") == "true";
    let attestor = if payroll_off { None } else { AuthorizationKey::from_env("AXOM_ATTESTATION_SIGNING_KEY").ok() };
    if payroll_off {
        tracing::info!("payroll disabled (AXOM_DISABLE_PAYROLL=true)");
    } else if attestor.is_none() {
        tracing::warn!("payroll disabled: {}", axom_app::signing_key_problem("AXOM_ATTESTATION_SIGNING_KEY"));
    }
    // Integration keys saved in the dashboard (encrypted with AXOM_SECRETS_KEY)
    // override .env; without that key, settings come from .env only.
    let secrets = match axom_core::secretbox::SecretBox::from_env() {
        Ok(b) => Some(b),
        Err(e) => {
            tracing::warn!("{e}: integration keys can only come from .env");
            None
        }
    };
    let rt_policy = std::sync::Arc::new(policy.clone());
    let perf = std::sync::Arc::new(perf);
    let attestor = attestor.map(std::sync::Arc::new);
    let price: std::sync::Arc<dyn PriceFeed> = std::sync::Arc::new(HttpPriceFeed::from_env());
    let rt_secrets = secrets.clone();
    let rt_pool = pool.clone();
    let make = move |config| axom_app::runtime::Runtime {
        label: "default".into(),
        pool: rt_pool.clone(),
        policy: rt_policy.clone(),
        perf: perf.clone(),
        notify: notify_cfg.clone(),
        config,
        attestor: attestor.clone(),
        network: axom_signer_network(),
        price: price.clone(),
        api_url: axom_app::agent_callback_url(),
        meet_web_url: env_or("AXOM_MEET_WEB_URL", "http://localhost:5173"),
        secrets: rt_secrets.clone(),
        tool_guard: axom_tools::adapters::mcp::UrlGuard::from_env(),
    };
    tokio::spawn(axom_app::single::supervise(pool.clone(), secrets.clone(), make));

    let bind = env_or("AXOM_API_BIND", "127.0.0.1:8080");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "serving API");
    // Approvals need a way to check dashboard sign-ins (Supabase) and the
    // approval signing key. Say exactly which one is missing or wrong.
    let approvals = match (axom_api::auth::AuthConfig::from_env(), AuthorizationKey::from_env("AXOM_APPROVAL_SIGNING_KEY")) {
        (Some(auth), Ok(key)) => Some(axom_api::ApprovalConfig { auth, policy: std::sync::Arc::new(policy.clone()), approval_key: std::sync::Arc::new(key) }),
        (auth, key) => {
            if auth.is_none() {
                tracing::warn!("approval endpoints disabled: set SUPABASE_JWKS_URL (or the legacy SUPABASE_JWT_SECRET) so the API can check dashboard sign-ins");
            }
            if key.is_err() {
                tracing::warn!("approval endpoints disabled: {}", axom_app::signing_key_problem("AXOM_APPROVAL_SIGNING_KEY"));
            }
            None
        }
    };
    let api = axom_app::single::LiveApi::new(pool, approvals, std::sync::Arc::new(policy.clone()), secrets);
    axum::serve(listener, api.into_router()).await?;
    Ok(())
}

/// `testnet` unless NEAR_NETWORK=mainnet is set explicitly.
fn axom_signer_network() -> String {
    match std::env::var("NEAR_NETWORK").as_deref() {
        Ok("mainnet") => "mainnet".into(),
        _ => "testnet".into(),
    }
}

async fn connect() -> anyhow::Result<sqlx::PgPool> {
    let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
    let opts = PgConnectOptions::from_str(&db_url).context("parsing DATABASE_URL")?.options([("search_path", "public")]);
    let pool = PgPoolOptions::new().max_connections(10).connect_with(opts).await.context("connecting to Postgres")?;
    axom_core::MIGRATOR.run(&pool).await.context("running migrations")?;
    Ok(pool)
}

/// `axom demo`: the judge-mode scenario. External services are mocks except
/// NEAR testnet: the small payout is signed by the running axom-signer
/// (OpenBao) and sent on testnet.
async fn demo_main() -> anyhow::Result<()> {
    use axom_app::demo;
    use std::sync::Arc;
    let pool = connect().await?;
    let root = PathBuf::from(env_or("AXOM_ROOT", "."));
    let c = demo::Configs::load(&root)?;
    let out_dir = root.join("judge/out");
    std::fs::create_dir_all(&out_dir)?;
    let now = chrono::Utc::now();
    let mocks = Arc::new(MockTools::new());
    let intake = axom_api::intake::DbTaskIntake { pool: pool.clone() };
    let notify_cfg = NotifyConfig::from_env();
    let step = |n: u32, t: &str| println!("\n==> {n}. {t}");

    // Judge mode: a dashboard admin the judge can act as (approve.sh).
    if let Ok(id) = std::env::var("AXOM_JUDGE_ADMIN_ID") {
        sqlx::query("INSERT INTO axom.admins (user_id, email, name) VALUES ($1::uuid, 'judge@judge.example', 'Judge') ON CONFLICT DO NOTHING")
            .bind(id)
            .execute(&pool)
            .await?;
    }

    step(1, "Boot as HR from the master plan; route tasks; run mock tools");
    let seed = demo::seed(&pool, &c, mocks.as_ref(), &intake, now).await?;
    println!("    plan {} | {} tasks assigned | {} hiring request(s) | {} done by mock tools | seeded {} USD of earlier payouts this month",
        seed.plan_id, seed.tasks_assigned, seed.hiring_requests, seed.tasks_done_by_mock_tools, seed.historical_payouts_usd);

    step(2, "Hiring: AI requests hire autonomously; the human role posts to Upwork (mock) after a policy check");
    for (id, r) in axom_hr::hiring::process_open_requests(&pool, &c.policy, mocks.as_ref()).await? {
        println!("    hiring request {id}: {r}");
    }

    step(3, "Interview: AI interviewer (real interview logic) with disclosure and consent");
    let port = env_or("AXOM_DEMO_API_PORT", "18181");
    let api_url = format!("http://127.0.0.1:{port}");
    let services = axom_api::Services {
        policy: Arc::new(c.policy.clone()),
        notify: notify_cfg.clone(),
        rooms: mocks.clone(),
        mailer: mocks.clone(),
        crm: mocks.clone(),
        livekit_ws_url: "ws://localhost:7880 (mock)".into(),
        public_api_url: api_url.clone(),
        meet_web_url: env_or("AXOM_MEET_WEB_URL", "http://localhost:5173"),
        agent_keys: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    let app = axom_api::router(axom_api::AppState { services: Some(Arc::new(services)), ..axom_api::AppState::new(pool.clone()) });
    tokio::spawn(async move { axum::serve(listener, app).await });
    let m2 = mocks.clone();
    let (meeting_id, metadata) = demo::interview(&pool, &c, mocks.as_ref(), &api_url, move || {
        m2.calls_to("livekit").into_iter().rev().find(|x| x.method == "dispatch_agent").and_then(|x| x.args["metadata"].as_str().map(str::to_string))
    })
    .await?;
    let meta_file = out_dir.join("interview-job.json");
    std::fs::write(&meta_file, metadata.to_string())?;
    let sim = tokio::process::Command::new(env_or("PYTHON", "python3"))
        .args(["-m", "axom_meet.simulate", "--metadata"])
        .arg(std::fs::canonicalize(&meta_file)?)
        .current_dir(root.join("meet-agent"))
        .output()
        .await
        .context("running the interview simulator (python3)")?;
    for line in String::from_utf8_lossy(&sim.stdout).lines() {
        println!("    {line}");
    }
    anyhow::ensure!(sim.status.success(), "interview simulator failed: {}", String::from_utf8_lossy(&sim.stderr));
    println!("    meeting {meeting_id}: consent and transcript recorded");

    step(4, "Performance: the seeded low performer is fired after two below-threshold periods");
    let fired = demo::fire_low_performer(&pool, &c, &notify_cfg, seed.low_performer, now).await?;
    println!("    fired {} | {} open task(s) reopened | owner notification {}", fired.worker_id, fired.reopened_tasks.len(), fired.notification_id);

    step(5, "Payroll: payouts under the threshold, at/over the threshold, and over the monthly cap");
    let attestor = AuthorizationKey::from_env("AXOM_ATTESTATION_SIGNING_KEY").context("AXOM_ATTESTATION_SIGNING_KEY (see `axom keygen`)")?;
    // Judge mode pins a testnet-only rate so faucet funds cover the demo
    // transfers (testnet NEAR has no value). Production uses the live feed.
    let pinned = std::env::var("AXOM_DEMO_USD_PER_NEAR").ok().and_then(|v| v.parse::<rust_decimal::Decimal>().ok());
    let price = match pinned {
        Some(rate) => {
            println!("    judge mode: testnet rate pinned at {rate} USD/NEAR so faucet funds cover the demo (production uses the live price feed)");
            Ok(axom_core::decision::PriceQuote { usd_per_near: rate, fetched_at: chrono::Utc::now() })
        }
        None => HttpPriceFeed::from_env().quote().await,
    };
    let price = match price {
        Ok(q) => q,
        Err(e) => {
            println!("    price feed unavailable ({e}); judge mode uses a fixed 3 USD/NEAR");
            axom_core::decision::PriceQuote { usd_per_near: rust_decimal::Decimal::from(3), fetched_at: chrono::Utc::now() }
        }
    };
    println!("    NEAR price: {} USD", price.usd_per_near);
    let p = demo::payouts(&pool, &c, &attestor, price, &axom_signer_network(), chrono::Utc::now()).await?;
    println!("    small payout {} -> {} (approved automatically)", p.small, p.small_recipient);
    println!("    held payout  {} (pending_approval: approve it in the dashboard)", p.held);
    println!("    blocked      {} (cap_exceeded; owner notified)", p.blocked);
    println!(
        "    cross-chain  {} -> {} on {} via NEAR Intents (approved automatically)",
        p.cross_chain,
        p.cross_chain_destination.address,
        axom_core::intents::chain_name(&p.cross_chain_destination.chain)
    );
    // NEAR Intents has no testnet: show a real mainnet dry-run quote for the
    // same route (no funds move). The testnet run simulates only the solver.
    let (amount_usd,): (rust_decimal::Decimal,) = sqlx::query_as("SELECT amount_usd FROM axom.payouts WHERE id = $1").bind(p.cross_chain).fetch_one(&pool).await?;
    match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        intents_dry_quote(&p.cross_chain_destination.asset_id, &p.cross_chain_destination.address, amount_usd, live_near_price().await),
    )
    .await
    {
        Ok(Ok(q)) => println!(
            "    NEAR Intents mainnet dry quote for this route: {} wNEAR -> {} USDC on Base (~{} USD, ~{}s)",
            q["amount_in_near"].as_str().unwrap_or("?"),
            q["amount_out"].as_str().unwrap_or("?"),
            q["amount_out_usd"].as_str().unwrap_or("?"),
            q["time_estimate_secs"]
        ),
        Ok(Err(e)) => println!("    NEAR Intents dry quote unavailable: {e}"),
        Err(_) => println!("    NEAR Intents dry quote timed out"),
    }

    step(6, "Waiting for axom-signer to sign (OpenBao) and send the small payout and the cross-chain deposit on NEAR testnet");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(env_or("AXOM_DEMO_WAIT_SECS", "180").parse().unwrap_or(180));
    let mut tx_hash: Option<String> = None;
    loop {
        let (status, hash, err): (String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT status, tx_hash, last_error FROM axom.payouts WHERE id = $1").bind(p.small).fetch_one(&pool).await?;
        if status == "finalized" || status == "sent" {
            tx_hash = hash;
            if status == "finalized" {
                break;
            }
        }
        if std::time::Instant::now() > deadline {
            println!("    timed out waiting (status {status}{})", err.map(|e| format!(", last error: {e}")).unwrap_or_default());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    if let Some(h) = &tx_hash {
        println!("    NEAR testnet transaction: {h}");
        println!("    https://testnet.nearblocks.io/txns/{h}");
    }
    // The cross-chain payout: wrap + transfer to the intent's deposit address on wrap.testnet.
    let xc_hash: Option<String> = loop {
        let (status, hash, err, intent): (String, Option<String>, Option<String>, Option<String>) =
            sqlx::query_as("SELECT status, tx_hash, last_error, intent_status FROM axom.payouts WHERE id = $1").bind(p.cross_chain).fetch_one(&pool).await?;
        if status == "finalized" {
            println!("    cross-chain payout finalized (NEAR Intents status {})", intent.unwrap_or_default());
            break hash;
        }
        if std::time::Instant::now() > deadline {
            println!("    cross-chain payout is {status}{}", err.map(|e| format!(" (last error: {e})")).unwrap_or_default());
            break hash;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    };
    if let Some(h) = &xc_hash {
        println!("    NEAR testnet deposit into the intent (wrap.testnet: storage, near_deposit, ft_transfer): {h}");
        println!("    https://testnet.nearblocks.io/txns/{h}");
    }

    step(7, "Final report");
    let report = demo::final_report(&pool, chrono::Utc::now()).await?;
    let json = serde_json::to_string_pretty(&serde_json::json!({ "report": report, "testnet_tx_hash": tx_hash, "cross_chain_deposit_tx_hash": xc_hash }))?;
    std::fs::write(out_dir.join("report.json"), &json)?;
    println!("    cap {} USD | committed {} | reserved {} | remaining {}", report.cap_usd, report.committed_usd, report.reserved_usd, report.remaining_usd);
    println!("    sent: {} | cross-chain: {} | waiting for approval: {} | blocked: {} | fired: {} | interviews: {}",
        report.sent.len(), report.cross_chain.len(), report.waiting_for_approval.len(), report.blocked.len(), report.fired.len(), report.interviews.len());
    println!("    full report: {}", out_dir.join("report.json").display());
    anyhow::ensure!(tx_hash.is_some(), "the testnet payout did not go out; check the signer log");
    Ok(())
}

/// Live NEAR/USD for mainnet dry quotes (the judge-mode testnet rate is pinned
/// and meaningless on mainnet): the price feed, else 1Click's own wNEAR
/// price, else 3 USD/NEAR.
async fn live_near_price() -> rust_decimal::Decimal {
    use axom_tools::intents::{IntentsApi, OneClickApi};
    if let Ok(q) = HttpPriceFeed::from_env().quote().await {
        return q.usd_per_near;
    }
    if let Ok(tokens) = OneClickApi::from_env().tokens().await {
        if let Some(p) = tokens.iter().find(|t| t.asset_id == "nep141:wrap.near").and_then(|t| t.price) {
            if let Some(d) = rust_decimal::Decimal::from_f64_retain(p) {
                return d.round_dp(6);
            }
        }
    }
    rust_decimal::Decimal::from(3)
}

/// A NEAR Intents (1Click, mainnet) `dry: true` quote: what paying `usd` in
/// wNEAR to `address` as `asset_id` would deliver. No funds move.
async fn intents_dry_quote(asset_id: &str, address: &str, usd: rust_decimal::Decimal, usd_per_near: rust_decimal::Decimal) -> anyhow::Result<serde_json::Value> {
    use axom_tools::intents::{IntentsApi, OneClickApi, QuoteRequest};
    let yocto = axom_core::near::usd_to_yocto(usd, usd_per_near)?;
    let api = OneClickApi::from_env();
    // Dry quotes never refund, but 1Click still wants a plain account here
    // (it rejects contracts such as wrap.near). Real quotes refund to the
    // treasury's implicit account.
    let refund_to = std::env::var("AXOM_INTENTS_DRY_REFUND_TO").unwrap_or_else(|_| "a".repeat(64));
    let req = QuoteRequest::payout(
        axom_core::intents::origin_asset("mainnet"),
        asset_id.to_string(),
        address.to_string(),
        yocto,
        refund_to,
        100,
        chrono::Utc::now() + chrono::Duration::hours(1),
        true,
    );
    let q = api.quote(&req).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(serde_json::json!({
        "route": format!("nep141:wrap.near -> {asset_id}"),
        "recipient": address,
        "usd": usd.to_string(),
        "usd_per_near": usd_per_near.to_string(),
        "amount_in_yocto": q.amount_in,
        "amount_in_near": axom_core::near::yocto_to_near_string(yocto),
        "amount_out": q.amount_out_formatted,
        "amount_out_usd": q.amount_out_usd.map(|u| u.to_string()),
        "min_amount_out": q.min_amount_out,
        "time_estimate_secs": q.time_estimate_secs,
    }))
}

async fn intents_tokens(chain: Option<String>) -> anyhow::Result<()> {
    use axom_tools::intents::{IntentsApi, OneClickApi};
    let tokens = OneClickApi::from_env().tokens().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    for t in tokens.iter().filter(|t| chain.as_deref().is_none_or(|c| t.blockchain == c)) {
        println!("{:<8} {:<10} {}", t.blockchain, t.symbol, t.asset_id);
    }
    Ok(())
}
