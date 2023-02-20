use std::collections::BTreeMap;

use anyhow::{anyhow, bail};
use serde::{Serialize, Deserialize};
use scraper::{Selector, Html};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TorrentInfo {
    name: String,
    description: String,
    infohash: String,
    category: String,
    ty: String,
    language: String,
    total_size: u64,
    uploader: String,
    downloads: usize,
    last_checked_ts: u64,
    uploaded_ts: u64,
    seeders: usize,
    leechers: usize,
    scraped_ts: u64,
    tmdb_id: Option<usize>,
    images: Vec<String>,
    trackers: Vec<String>,
    files: Vec<File>,
    comments: Vec<Comment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct File {
    name: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawComment {
    avatar: String,
    class: String,
    comment: String,
    commentid: u64,
    posted: String,
    username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Comment {
    avatar: String,
    class: String,
    comment: String,
    commentid: u64,
    posted: u64,
    username: String,
}

/// Transforms stuff like "1 year ago" into a timestamp
fn parse_time_offset(now: u64, value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let parts = value.split(' ').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }

    let number = match parts[0].parse::<u64>() {
        Ok(number) => number,
        Err(_) => return None,
    };
    let unit = parts[1];
    let ago = parts[2];

    if ago != "ago" {
        return None;
    }

    match unit.trim_end_matches('s') {
        "second" => Some(now - number),
        "minute" => Some(now - number * 60),
        "hour" => Some(now - number * 60 * 60),
        "day" => Some(now - number * 86400),
        "week" => Some(now - number * 86400 * 7),
        "month" => Some(now - number * 86400 * 30),
        "year" => Some(now - number * 86400 * 365),
        "decade" => Some(now - number * 86400 * 365 * 10),
        _ => None,
    }
}

/// Transforms formatted size like "87.8 MB" or "742.2 KB" into bytes
fn parse_data_size(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let parts = value.split(' ').collect::<Vec<_>>();
    if parts.len() != 2 {
        return None;
    }

    let number = match parts[0].replace(',', "").parse::<f64>() {
        Ok(number) => number,
        Err(_) => return None,
    };
    let unit = parts[1];

    Some(match unit.trim_end_matches('s') {
        "B" => number as u64,
        "KB" => (number * 1024.0) as u64,
        "MB" => (number * 1024.0 * 1024.0) as u64,
        "GB" => (number * 1024.0 * 1024.0 * 1024.0) as u64,
        "TB" => (number * 1024.0 * 1024.0 * 1024.0 * 1024.0) as u64,
        _ => return None,
    })
}

/// Transforms a file like "File(2) Name (1.2 GB)" into a File struct
fn parse_file(value: &str) -> Option<File> {
    let value = value.trim();
    if value.is_empty() || !value.ends_with(')') {
        return None;
    }

    let index = value.rfind('(')?;
    let name = value[..index].trim().to_string();
    let size = parse_data_size(&value[index+1..value.len()-1])?;

    Some(File { name, size })
}

fn scrape_torrent(id: usize) -> Result<Option<TorrentInfo>, anyhow::Error> {
    let url = format!("https://1337x.torrentbay.to/torrent/{id}/friendly-scraper/");
    let resp = minreq::get(url).send()?;
    let body = resp.as_str()?;
    if resp.status_code != 200 {
        bail!("Unexpected status code {}: {} {}", id, resp.status_code, body);
    }

    let now = chrono::Utc::now().timestamp() as u64;
    let document = Html::parse_document(body);

    // Scrape general information
    let list_selector = Selector::parse(".list").unwrap();
    let span_selector = Selector::parse("span").unwrap();
    let lists = document.select(&list_selector).collect::<Vec<_>>();
    if lists.len() != 3 {
        if body.contains("Bad Torrent ID.") {
            return Ok(None);
        }
        println!("{body}");
        bail!("Unexpected number of lists: {}", lists.len());
    }
    let mut spans = lists[1].select(&span_selector).collect::<Vec<_>>();
    spans.extend(lists[2].select(&span_selector));
    if spans.len() != 10 {
        bail!("Unexpected number of spans: {}", spans.len());
    }
    let category = spans[0].text().next().unwrap_or_default().to_string();
    let ty = spans[1].text().next().unwrap_or_default().to_string();
    let language = spans[2].text().next().unwrap_or_default().to_string();
    let total_size = spans[3].text().next().unwrap_or_default().to_string();
    let total_size = parse_data_size(&total_size).ok_or_else(|| anyhow!("Invalid size: {}", total_size))?;
    let uploader = spans[4].text().map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or_default().to_string();
    let downloads = spans[5].text().next().unwrap_or_default().to_string();
    let downloads = downloads.parse().map_err(|_| anyhow!("Invalid downloads: {}", downloads))?;
    let last_checked = spans[6].text().next().unwrap_or_default();
    let last_checked_ts = parse_time_offset(now, last_checked).ok_or_else(|| anyhow!("Invalid last checked: {last_checked:?}"))?;
    let uploaded = spans[7].text().next().unwrap_or_default();
    let uploaded_ts = parse_time_offset(now, uploaded).ok_or_else(|| anyhow!("Invalid uploaded: {uploaded:?}"))?;
    let seeders: usize = spans[8].text().next().unwrap_or_default().to_string().parse()?;
    let leechers: usize = spans[9].text().next().unwrap_or_default().to_string().parse()?;

    // Scrape TMDB id
    let movie_link_selector = Selector::parse(".torrent-detail-info h3>a").unwrap();
    let movie_link = document.select(&movie_link_selector).next().and_then(|link| {
        link.value().attr("href").map(|href| href.to_string())
    });
    let tmdb_id = 'tbdb_id: {match movie_link {
        Some(movie_link) => {
            if !movie_link.starts_with("/movie/") {
                eprintln!("Unexpected movie link: {movie_link}");
                break 'tbdb_id None;
            }

            let parts = movie_link.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>();
            if parts.len() != 3 {
                eprintln!("Unexpected movie link: {movie_link}");
                break 'tbdb_id None;
            }

            match parts[1].parse::<usize>() {
                Ok(tmdb_id) => Some(tmdb_id),
                Err(err) => {
                    eprintln!("Unexpected movie link: {movie_link} ({err})");
                    break 'tbdb_id None;
                }
            }
        }
        None => None,
    }};

    // Scrape infohash
    let infohash_selector = Selector::parse(".infohash-box>p>span").unwrap();
    let infohash_el = document.select(&infohash_selector).next().ok_or_else(|| anyhow::anyhow!("No infohash found"))?;
    let infohash = infohash_el.text().next().unwrap_or_default().to_string();

    // Scrape name and description
    let h1_selector = Selector::parse("h1").unwrap();
    let h1 = document.select(&h1_selector).next().ok_or_else(|| anyhow::anyhow!("No h1 found"))?;
    let mut name = h1.text().next().unwrap_or_default().trim().to_string();
    let mut name_incomplete = false;
    if name.ends_with("...") {
        name.pop();
        name.pop();
        name.pop();
        name_incomplete = true;
    }
    let description_selector = Selector::parse(".torrent-tabs #description").unwrap();
    let description_el = document.select(&description_selector).next().ok_or_else(|| anyhow::anyhow!("No description found"))?;
    let mut description_parts = description_el.text().map(|t| t.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>();
    if description_parts.len() == 1 && description_parts[0] == "No description given." {
        description_parts.clear();
    }
    let mut description = description_parts.join("\n");
    if (name_incomplete && description.starts_with(&name)) || (!name_incomplete && description.starts_with(&format!("{name}\n"))) {
        name = description.lines().next().unwrap_or_default().to_string();
        description = description.lines().skip(1).collect::<Vec<_>>().join("\n");
    }

    // Scrape images
    let image_selector = Selector::parse(".torrent-tabs #description img").unwrap();
    let images = document.select(&image_selector).filter_map(|img| {
        img.value().attr("data-original").map(|src| src.to_string())
    }).collect::<Vec<_>>();

    // Scrape trackers
    let tracker_selector = Selector::parse(".torrent-tabs #tracker-list li").unwrap();
    let trackers = document.select(&tracker_selector)
        .map(|li| li.text().next().unwrap_or_default().trim().to_string())
        .collect::<Vec<_>>();

    // Scrape files
    let file_selector = Selector::parse(".torrent-tabs #files li").unwrap();
    let raw_files = document.select(&file_selector)
        .map(|li| li.text().next().unwrap_or_default().trim().to_string())
        .collect::<Vec<_>>();
    let mut files: Vec<File> = Vec::new();
    for raw_file in raw_files {
        match parse_file(&raw_file) {
            Some(file) => files.push(file),
            None => eprintln!("Failed to parse file: {raw_file}"),
        }
    }

    // Scrape comments
    let comment_count_selector = Selector::parse(".torrent-tabs .tab-nav a[href=\"#comments\"]>span").unwrap();
    let comment_count = document.select(&comment_count_selector).next().and_then(|span| {
        span.text().next().and_then(|text| text.parse::<usize>().ok())
    }).unwrap_or_default();
    let mut comments: Vec<Comment> = Vec::new();
    if comment_count > 0 {
        let comments_url = format!("https://1337x.torrentbay.to/comments.php?torrentid={id}");
        let comments_resp = minreq::get(comments_url).send()?;
        let comments_body = comments_resp.as_str()?;
        if comments_resp.status_code != 200 {
            bail!("Unexpected status code for comments {}: {} {}", id, comments_resp.status_code, comments_body);
        }
        let raw_comments: Vec<RawComment> = serde_json::from_str(comments_body)?;
        for raw_comment in raw_comments {
            let posted = match parse_time_offset(now, &raw_comment.posted) {
                Some(posted) => posted,
                None => {
                    eprintln!("Failed to parse comment posted time: {}", raw_comment.posted);
                    continue;
                }
            };
            let comment = Comment {
                avatar: raw_comment.avatar,
                class: raw_comment.class,
                comment: raw_comment.comment,
                commentid: raw_comment.commentid,
                posted,
                username: raw_comment.username,
            };
            comments.push(comment);
        }
        if comments.is_empty() {
            eprintln!("No comments found for {id}");
        }
    }

    Ok(Some(TorrentInfo {
        name,
        description,
        ty,
        category,
        images,
        trackers,
        files,
        comments,
        infohash,
        language,
        total_size,
        uploader,
        downloads,
        last_checked_ts,
        uploaded_ts,
        seeders,
        leechers,
        scraped_ts: now,
        tmdb_id,
    }))
}

fn main() {
    let data = std::fs::read_to_string("data.json").unwrap();
    let mut data: BTreeMap<usize, Option<TorrentInfo>> = serde_json::from_str(&data).unwrap();
    let mut i = 99;
    loop {
        i += 1;

        if data.contains_key(&i) {
            continue;
        }

        match scrape_torrent(i) {
            Ok(info) => {
                println!("Scraped torrent {i}: {}", info.as_ref().map(|i| i.name.as_str()).unwrap_or("none"));
                data.insert(i, info);
            }
            Err(err) => eprintln!("Failed to scrape torrent {i}: {err}"),
        }

        if i % 60 == 0 {
            println!("Saving data");
            let mut file = std::fs::File::create("data.json").unwrap();
            serde_json::to_writer_pretty(&mut file, &data).unwrap();
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
