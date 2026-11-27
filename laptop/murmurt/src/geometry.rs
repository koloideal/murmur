//! Probe the murmur island's geometry through waybar's AT-SPI tree.
//!
//! The island is a waybar group whose module labels sit in one GTK
//! container. We find the status label (its text starts with "murmur "),
//! then take every label in the same parent container — that's exactly
//! the island, no matter how many modules are visible. The pill's
//! horizontal padding is symmetric, so the text union's center is the
//! pill's center — no pixel scanning, no magic constants.

use anyhow::{Context, Result, bail};
use atspi::connection::AccessibilityConnection;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::{CoordType, ObjectRefOwned};
use serde::Serialize;
use zbus::proxy::CacheProperties;

#[derive(Serialize)]
struct ModuleGeo {
    name: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Serialize)]
struct IslandGeo {
    cx: i32,
    left: i32,
    right: i32,
    bar_bottom: i32,
    modules: Vec<ModuleGeo>,
}

async fn mk_accessible(
    conn: &zbus::Connection,
    objref: &ObjectRefOwned,
) -> Option<AccessibleProxy<'static>> {
    if objref.is_null() {
        return None;
    }
    let name = objref.name()?.clone();
    let path = objref.path().clone();
    AccessibleProxy::builder(conn)
        .destination(name)
        .ok()?
        .path(path)
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()
}

async fn accessible_children(
    conn: &zbus::Connection,
    node: &AccessibleProxy<'_>,
) -> Vec<(ObjectRefOwned, AccessibleProxy<'static>)> {
    let count = node.child_count().await.unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..count.min(300) {
        if let Ok(objref) = node.get_child_at_index(i).await {
            if let Some(proxy) = mk_accessible(conn, &objref).await {
                out.push((objref, proxy));
            }
        }
    }
    out
}

struct LabelHit {
    name: String,
    parent: ObjectRefOwned,
    grandparent: Option<ObjectRefOwned>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

async fn component_extents(conn: &zbus::Connection, objref: &ObjectRefOwned) -> Option<(i32, i32, i32, i32)> {
    let name = objref.name()?.clone();
    let path = objref.path().clone();
    let comp = ComponentProxy::builder(conn)
        .destination(name)
        .ok()?
        .path(path)
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()?;
    comp.get_extents(CoordType::Screen).await.ok()
}

async fn collect_labels(
    conn: &zbus::Connection,
    objref: &ObjectRefOwned,
    node: &AccessibleProxy<'_>,
    depth: usize,
    out: &mut Vec<LabelHit>,
) {
    if depth > 14 {
        return;
    }
    let role = node.get_role_name().await.unwrap_or_default();
    if role == "label" {
        if let Some((x, y, w, h)) = component_extents(conn, objref).await {
            let name = node.name().await.unwrap_or_default();
            let parent = node.parent().await.unwrap_or_default();
            // module labels live in per-module boxes inside the island's
            // group container — the grandparent is what siblings share
            let mut grandparent = None;
            if let Some(pproxy) = mk_accessible(conn, &parent).await {
                if let Ok(gp) = pproxy.parent().await {
                    if !gp.is_null() {
                        grandparent = Some(gp);
                    }
                }
            }
            out.push(LabelHit { name, parent, grandparent, x, y, w, h });
        }
        return;
    }
    for (childref, child) in accessible_children(conn, node).await {
        Box::pin(collect_labels(conn, &childref, &child, depth + 1, out)).await;
    }
}

pub async fn geometry() -> Result<()> {
    let atspi = AccessibilityConnection::new()
        .await
        .context("no AT-SPI bus — is a11y enabled in this session?")?;
    let conn = atspi.connection().clone();
    let root = atspi.root_accessible_on_registry().await?;

    // Desktop children are applications; find waybar.
    let mut waybar = None;
    for (_, app) in accessible_children(&conn, &root).await {
        let name = app.name().await.unwrap_or_default().to_lowercase();
        if name.contains("waybar") {
            waybar = Some(app);
            break;
        }
    }
    let Some(app) = waybar else {
        bail!("waybar not found on the AT-SPI bus — is it running?")
    };

    let mut hits: Vec<LabelHit> = Vec::new();
    for (childref, child) in accessible_children(&conn, &app).await {
        Box::pin(collect_labels(&conn, &childref, &child, 0, &mut hits)).await;
    }

    if std::env::var("MURMURT_DEBUG").is_ok() {
        for h in &hits {
            eprintln!(
                "label {:?} x={} w={} parent={} gp={}",
                h.name.chars().take(30).collect::<String>(),
                h.x,
                h.w,
                h.parent.path_as_str(),
                h.grandparent.as_ref().map(|g| g.path_as_str().to_string()).unwrap_or("-".into()),
            );
        }
    }

    // Anchor: the status label's text starts with "murmur ". Island
    // modules render edge-to-edge (padding lives inside each module
    // box), while the gap to anything outside the pill is the group
    // margin — so contiguity along x is the robust grouping signal,
    // no assumptions about the AT-SPI container hierarchy.
    let mut by_x: Vec<&LabelHit> = hits.iter().collect();
    by_x.sort_by_key(|h| h.x);
    let Some(ai) = by_x.iter().position(|h| h.name.starts_with("murmur ")) else {
        bail!("no murmur status label in waybar — island modules missing from the bar config?");
    };
    let gap_max = 4;
    let mut lo = ai;
    while lo > 0 {
        let gap = by_x[lo].x - (by_x[lo - 1].x + by_x[lo - 1].w);
        if gap > gap_max {
            break;
        }
        lo -= 1;
    }
    let mut hi = ai;
    while hi + 1 < by_x.len() {
        let gap = by_x[hi + 1].x - (by_x[hi].x + by_x[hi].w);
        if gap > gap_max {
            break;
        }
        hi += 1;
    }
    let mut modules: Vec<&LabelHit> = by_x[lo..=hi].to_vec();
    modules.sort_by_key(|h| h.x);

    let left = modules.iter().map(|h| h.x).min().unwrap();
    let right = modules.iter().map(|h| h.x + h.w).max().unwrap();
    let bar_bottom = modules.iter().map(|h| h.y + h.h).max().unwrap_or(40);

    let geo = IslandGeo {
        cx: (left + right) / 2,
        left,
        right,
        bar_bottom,
        modules: modules
            .iter()
            .map(|h| ModuleGeo {
                name: h.name.chars().take(40).collect(),
                x: h.x,
                y: h.y,
                w: h.w,
                h: h.h,
            })
            .collect(),
    };
    println!("{}", serde_json::to_string_pretty(&geo)?);
    Ok(())
}
