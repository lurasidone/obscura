use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use uuid::Uuid;
use wreq::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE, COOKIE, USER_AGENT};
use wreq_util::{Emulation, Platform, Profile};

use super::device::{
    jitter_u64, resolve_session_path, DeviceIdentity, SessionStore, API_BASE, API_DOMAIN,
};
use super::error::{IgError, IgResult};
use super::model::{
    ActivityFeedResponse, Collection, CollectionsResponse, Comment, CommentsResponse,
    CurrentUserResponse, DeleteMediaResponse, DirectThread, FeedResponse, FollowListResponse,
    Friendship, FriendshipDestroyResponse, InboxResponse, Media, MediaItem, StatusResponse,
    ThreadResponse, User, UserInfoResponse, UserShort,
};
use super::ratelimit::RateLimiter;

#[derive(Debug)]
struct LiveAuth {
    mid: String,
    ig_u_rur: String,
    ig_www_claim: String,
    authorization: String,
    user_id: String,
    username: String,
    sessionid: String,
    cookies: BTreeMap<String, String>,
}

pub struct InstagramClient {
    http: wreq::Client,
    device: DeviceIdentity,
    live: Mutex<LiveAuth>,
    rate_limiter: Arc<RateLimiter>,
    base_url: String,
    session_path: PathBuf,
}

impl InstagramClient {
    pub async fn new(
        sessionid: &str,
        base_url: Option<&str>,
        session_path: Option<&str>,
        rate_limiter: Arc<RateLimiter>,
    ) -> IgResult<Self> {
        let session_path = resolve_session_path(session_path);
        let provided = sessionid.trim();
        let mut store = SessionStore::load(&session_path).unwrap_or_default();
        let provided_cookies = parse_cookie_pairs(provided);
        let sessionid = if !provided.is_empty() {
            let sid = normalize_sessionid(provided)?;
            // older session files may hold the encoded form of this same session
            let prev = normalize_sessionid(&store.sessionid).unwrap_or_default();
            store.sessionid = sid.clone();
            if prev.is_empty() || prev != sid {
                store.user_id.clear();
                store.username.clear();
                store.mid.clear();
                store.ig_u_rur.clear();
                store.ig_www_claim.clear();
                store.cookies.clear();
            }
            store.cookies.extend(provided_cookies);
            let _ = store.ensure_device();
            sid
        } else if !store.sessionid.is_empty() {
            let _ = store.ensure_device();
            normalize_sessionid(&store.sessionid)?
        } else {
            return Err(IgError::BadSessionId);
        };
        store.cookies.insert("sessionid".into(), sessionid.clone());

        let user_id_hint = if !store.user_id.is_empty() {
            store.user_id.clone()
        } else if let Some(uid) = store.cookies.get("ds_user_id") {
            uid.clone()
        } else {
            user_id_from_sessionid(&sessionid)?
        };
        store
            .cookies
            .insert("ds_user_id".into(), user_id_hint.clone());
        if !store.mid.is_empty() {
            let mid = store.mid.clone();
            store.cookies.insert("mid".into(), mid);
        }

        let device = store.device.clone().unwrap_or_else(DeviceIdentity::fresh);

        let authorization = build_authorization(&user_id_hint, &sessionid);

        let base_url = base_url
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(API_BASE)
            .trim_end_matches('/')
            .to_string();

        // OkHttp4.12+Android is the only profile that still matches the mobile app's TLS/header order
        let emulation = Emulation::builder()
            .profile(Profile::OkHttp4_12)
            .platform(Platform::Android)
            .headers(false)
            .build();

        let http = wreq::Client::builder()
            .emulation(emulation)
            .timeout(Duration::from_secs(30))
            .build()?;

        let live = LiveAuth {
            mid: store.mid.clone(),
            ig_u_rur: store.ig_u_rur.clone(),
            ig_www_claim: if store.ig_www_claim.is_empty() {
                "0".into()
            } else {
                store.ig_www_claim.clone()
            },
            authorization,
            user_id: user_id_hint.clone(),
            username: store.username.clone(),
            sessionid: sessionid.clone(),
            cookies: store.cookies.clone(),
        };

        let client = Self {
            http,
            device,
            live: Mutex::new(live),
            rate_limiter,
            base_url,
            session_path,
        };

        match client.fetch_current_user().await {
            Ok(user) => {
                {
                    let mut live = client.live.lock().expect("live auth poisoned");
                    if !user.pk.is_empty() {
                        live.user_id = user.pk;
                    }
                    live.username = user.username;
                    live.authorization = build_authorization(&live.user_id, &live.sessionid);
                }
                client.persist_session();
            }
            Err(e) => {
                tracing::warn!("current_user failed ({e}); trying users/{{id}}/info/");
                let user = client.fetch_user_info(&user_id_hint).await?;
                {
                    let mut live = client.live.lock().expect("live auth poisoned");
                    live.user_id = user.pk;
                    live.username = user.username;
                    live.authorization = build_authorization(&live.user_id, &live.sessionid);
                }
                client.persist_session();
            }
        }

        Ok(client)
    }

    pub fn user_id(&self) -> String {
        self.live
            .lock()
            .expect("live auth poisoned")
            .user_id
            .clone()
    }

    pub fn username(&self) -> String {
        self.live
            .lock()
            .expect("live auth poisoned")
            .username
            .clone()
    }

    async fn fetch_current_user(&self) -> IgResult<User> {
        let resp = self.get("accounts/current_user/", &[]).await?;
        let body: CurrentUserResponse = parse_json(resp).await?;
        body.user
            .ok_or_else(|| IgError::Other("missing user in current_user".into()))
    }

    pub async fn fetch_user_info(&self, user_id: &str) -> IgResult<User> {
        let resp = self.get(&format!("users/{user_id}/info/"), &[]).await?;
        let body: UserInfoResponse = parse_json(resp).await?;
        body.user
            .ok_or_else(|| IgError::Other("missing user in users/info".into()))
    }

    pub async fn user_medias_page(
        &self,
        user_id: &str,
        max_id: Option<&str>,
        count: u32,
    ) -> IgResult<(Vec<Media>, Option<String>)> {
        let count_s = count.to_string();
        let uid = self.user_id();
        let rank_token = format!("{}_{}", uid, self.device.uuid);
        let mut params: Vec<(&str, &str)> = vec![
            ("count", count_s.as_str()),
            ("rank_token", rank_token.as_str()),
            ("ranked_content", "true"),
        ];
        if let Some(cursor) = max_id {
            if !cursor.is_empty() {
                params.push(("max_id", cursor));
            }
        }
        let resp = self.get(&format!("feed/user/{user_id}/"), &params).await?;
        let body: FeedResponse = parse_json(resp).await?;
        let next = body.next_max_id.filter(|s| !s.is_empty());
        Ok((body.items, next))
    }

    pub async fn media_delete(&self, media_id: &str) -> IgResult<bool> {
        let data = self.default_data(json!({ "media_id": media_id }));
        let resp = self
            .post_signed(&format!("media/{media_id}/delete/"), &data)
            .await?;
        let body: DeleteMediaResponse = parse_json(resp).await?;
        Ok(body.did_delete.unwrap_or(false))
    }

    pub async fn liked_medias_page(
        &self,
        max_id: Option<&str>,
    ) -> IgResult<(Vec<Media>, Option<String>)> {
        self.activity_feed_page("feed/liked/", max_id, true, false)
            .await
    }

    pub async fn saved_medias_page(
        &self,
        max_id: Option<&str>,
    ) -> IgResult<(Vec<Media>, Option<String>)> {
        self.activity_feed_page("feed/saved/posts/", max_id, true, false)
            .await
    }

    pub async fn archived_medias_page(
        &self,
        max_id: Option<&str>,
    ) -> IgResult<(Vec<Media>, Option<String>)> {
        self.activity_feed_page("feed/only_me_feed/", max_id, false, true)
            .await
    }

    async fn activity_feed_page(
        &self,
        path: &str,
        max_id: Option<&str>,
        include_igtv_preview: bool,
        archive_cursor: bool,
    ) -> IgResult<(Vec<Media>, Option<String>)> {
        let mut params: Vec<(&str, &str)> = Vec::new();
        if include_igtv_preview {
            params.push(("include_igtv_preview", "false"));
        }
        if let Some(cursor) = max_id.filter(|s| !s.is_empty()) {
            params.push(("max_id", cursor));
        }
        let resp = self.get(path, &params).await?;
        let body: ActivityFeedResponse = parse_json(resp).await?;
        let next = if archive_cursor {
            body.max_id.clone().filter(|s| !s.is_empty())
        } else {
            body.next_cursor()
        };
        let items = body.items.into_iter().map(MediaItem::into_media).collect();
        Ok((items, next))
    }

    pub async fn collections(&self) -> IgResult<Vec<Collection>> {
        const TYPES: &str = r#"["ALL_MEDIA_AUTO_COLLECTION","PRODUCT_AUTO_COLLECTION","MEDIA"]"#;
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut params: Vec<(&str, &str)> = vec![("collection_types", TYPES)];
            if let Some(c) = cursor.as_deref().filter(|s| !s.is_empty()) {
                params.push(("max_id", c));
            }
            let resp = self.get("collections/list/", &params).await?;
            let body: CollectionsResponse = parse_json(resp).await?;
            out.extend(body.items);
            let next = body
                .next_max_id
                .or(body.max_id)
                .filter(|s| !s.is_empty() && Some(s.as_str()) != cursor.as_deref());
            match next {
                Some(n) if body.more_available => cursor = Some(n),
                _ => break,
            }
        }
        Ok(out)
    }

    pub async fn collection_medias_page(
        &self,
        collection_id: &str,
        max_id: Option<&str>,
    ) -> IgResult<(Vec<Media>, Option<String>)> {
        self.activity_feed_page(
            &format!("feed/collection/{collection_id}/"),
            max_id,
            true,
            false,
        )
        .await
    }

    pub async fn media_unlike(&self, media_id: &str) -> IgResult<bool> {
        let data = self.action_data(json!({
            "inventory_source": "media_or_ad",
            "media_id": media_id,
            "delivery_class": "organic",
            "tap_source": "button",
            "is_2m_enabled": "false",
            "is_from_swipe": "false",
            "is_carousel_bumped_post": "false",
            "floating_context_items": "[]",
            "media_pct_watched": "0",
            "container_module": "feed_timeline",
            "feed_position": jitter_u64(0, 6).to_string(),
        }));
        self.post_status(&format!("media/{media_id}/unlike/"), &data)
            .await
    }

    pub async fn media_unsave(&self, media_id: &str) -> IgResult<bool> {
        let data = self.action_data(json!({
            "media_id": media_id,
            "module_name": "feed_timeline",
        }));
        self.post_status(&format!("media/{media_id}/unsave/"), &data)
            .await
    }

    pub async fn media_unsave_from_collection(
        &self,
        media_id: &str,
        collection_id: &str,
    ) -> IgResult<bool> {
        let data = self.action_data(json!({
            "media_id": media_id,
            "module_name": "feed_timeline",
            "added_collection_ids": format!("[{collection_id}]"),
        }));
        self.post_status(&format!("media/{media_id}/unsave/"), &data)
            .await
    }

    /// Create Instagram's native repost/media note, following instagrapi's
    /// `media_note_create` payload and unsigned form request.
    pub async fn media_note_create(
        &self,
        media_id: &str,
        text: &str,
        extra_data: Option<Value>,
    ) -> IgResult<Value> {
        self.media_note_create_with_options(media_id, text, 7, 13, extra_data)
            .await
    }

    /// Create a media note/repost with the same configurable audience and
    /// style accepted by instagrapi's `media_note_create`.
    pub async fn media_note_create_with_options(
        &self,
        media_id: &str,
        text: &str,
        audience: i64,
        note_style: i64,
        extra_data: Option<Value>,
    ) -> IgResult<Value> {
        let mut data = json!({
            "inventory_source": "recommended_clips_chaining_model",
            "media_client_position": "0",
            "media_id": media_id,
            "note_style": note_style.to_string(),
            "carousel_index": "-1",
            "text": text,
            "_uuid": self.device.uuid,
            "audience": audience.to_string(),
            "event_source": "ufi",
            "container_module": "clips_viewer_clips_tab",
        });
        merge_object(&mut data, extra_data);
        let resp = self.post_form("media/create_note/v2/", &data).await?;
        parse_json(resp).await
    }

    /// Remove a native repost/media note by its note ID (not the media ID).
    pub async fn media_note_delete(
        &self,
        note_id: &str,
        extra_data: Option<Value>,
    ) -> IgResult<bool> {
        let mut data = json!({
            "inventory_source": "recommended_clips_chaining_model",
            "carousel_index": "-1",
            "_uuid": self.device.uuid,
            "event_source": "ufi",
            "container_module": "clips_viewer_clips_tab",
            "note_id": note_id,
        });
        merge_object(&mut data, extra_data);
        let resp = self.post_form("media/delete_note/", &data).await?;
        let body: StatusResponse = parse_json(resp).await?;
        Ok(body.status.as_deref() == Some("ok"))
    }

    pub async fn media_comments_page(
        &self,
        media_id: &str,
        max_id: Option<&str>,
    ) -> IgResult<(Vec<Comment>, Option<String>)> {
        let mut params: Vec<(&str, &str)> = vec![
            ("can_support_threading", "true"),
            ("permalink_enabled", "false"),
        ];
        if let Some(cursor) = max_id.filter(|s| !s.is_empty()) {
            if let Some(value) = cursor.strip_prefix("min:") {
                params.push(("min_id", value));
            } else {
                params.push(("max_id", cursor.strip_prefix("max:").unwrap_or(cursor)));
            }
        }
        let resp = self
            .get(&format!("media/{media_id}/comments/"), &params)
            .await?;
        let body: CommentsResponse = parse_json(resp).await?;
        let next = if body.has_more_comments {
            body.next_max_id
                .filter(|s| !s.is_empty())
                .map(|id| format!("max:{id}"))
        } else if body.has_more_headload_comments {
            body.next_min_id
                .filter(|s| !s.is_empty())
                .map(|id| format!("min:{id}"))
        } else {
            None
        };
        Ok((body.comments, next))
    }

    pub async fn comments_bulk_delete(
        &self,
        media_id: &str,
        comment_ids: &[String],
    ) -> IgResult<bool> {
        let joined = comment_ids.join(",");
        let data = self.action_data(json!({
            "comment_ids_to_delete": joined,
            "container_module": "self_comments_v2_newsfeed_you",
        }));
        self.post_status(&format!("media/{media_id}/comment/bulk_delete/"), &data)
            .await
    }

    pub async fn followers_page(
        &self,
        user_id: &str,
        max_id: Option<&str>,
    ) -> IgResult<(Vec<UserShort>, Option<String>)> {
        self.follow_list_page(user_id, "followers", max_id).await
    }

    pub async fn following_page(
        &self,
        user_id: &str,
        max_id: Option<&str>,
    ) -> IgResult<(Vec<UserShort>, Option<String>)> {
        self.follow_list_page(user_id, "following", max_id).await
    }

    async fn follow_list_page(
        &self,
        user_id: &str,
        list: &str,
        max_id: Option<&str>,
    ) -> IgResult<(Vec<UserShort>, Option<String>)> {
        let rank_token = format!("{}_{}", self.user_id(), self.device.uuid);
        let mut params: Vec<(&str, &str)> = vec![
            ("count", "200"),
            ("rank_token", rank_token.as_str()),
            ("search_surface", "follow_list_page"),
            ("query", ""),
            ("enable_groups", "true"),
        ];
        if let Some(cursor) = max_id.filter(|s| !s.is_empty()) {
            params.push(("max_id", cursor));
        }
        let resp = self
            .get(&format!("friendships/{user_id}/{list}/"), &params)
            .await?;
        let body: FollowListResponse = parse_json(resp).await?;
        Ok((body.users, body.next_max_id.filter(|s| !s.is_empty())))
    }

    pub async fn friendship(&self, user_id: &str) -> IgResult<Friendship> {
        let resp = self
            .get(
                &format!("friendships/show/{user_id}/"),
                &[("is_external_deeplink_profile_view", "false")],
            )
            .await?;
        parse_json(resp).await
    }

    pub async fn user_unfollow(&self, user_id: &str) -> IgResult<bool> {
        let data = self.action_data(json!({
            "user_id": user_id,
            "container_module": "profile",
        }));
        let resp = self
            .post_signed(&format!("friendships/destroy/{user_id}/"), &data)
            .await?;
        let body: FriendshipDestroyResponse = parse_json(resp).await?;
        Ok(body.friendship_status.and_then(|s| s.following) == Some(false))
    }

    pub async fn direct_inbox_page(
        &self,
        cursor: Option<&str>,
        thread_message_limit: u32,
    ) -> IgResult<(Vec<DirectThread>, Option<String>)> {
        let limit = thread_message_limit.to_string();
        let mut params: Vec<(&str, &str)> = vec![
            ("visual_message_return_type", "unseen"),
            ("thread_message_limit", limit.as_str()),
            ("persistentBadging", "true"),
            ("limit", "20"),
            ("is_prefetching", "false"),
            ("include_old_mrs", "false"),
            ("no_pending_badge", "true"),
            ("push_disabled", "true"),
        ];
        let tracking_id = Uuid::new_v4().to_string();
        params.push(("eb_device_id", "0"));
        params.push(("igd_request_log_tracking_id", tracking_id.as_str()));
        if let Some(c) = cursor.filter(|s| !s.is_empty()) {
            params.push(("cursor", c));
            params.push(("direction", "older"));
            params.push(("fetch_reason", "page_scroll"));
        } else {
            params.push(("fetch_reason", "initial_snapshot"));
        }
        let resp = self.get("direct_v2/inbox/", &params).await?;
        let body: InboxResponse = parse_json(resp).await?;
        let next = if body.inbox.has_older {
            body.inbox.oldest_cursor.filter(|s| !s.is_empty())
        } else {
            None
        };
        Ok((body.inbox.threads, next))
    }

    pub async fn direct_thread(
        &self,
        thread_id: &str,
        amount: u32,
        cursor: Option<&str>,
    ) -> IgResult<DirectThread> {
        // instagrapi's private endpoint requests 20 items per page and advances
        // with oldest_cursor; callers can still choose a smaller page.
        let limit = amount.clamp(1, 20).to_string();
        let mut params: Vec<(&str, &str)> = vec![
            ("visual_message_return_type", "unseen"),
            ("limit", limit.as_str()),
            ("direction", "older"),
            ("seq_id", "40065"),
        ];
        if let Some(c) = cursor.filter(|s| !s.is_empty()) {
            params.push(("cursor", c));
        }
        let resp = self
            .get(&format!("direct_v2/threads/{thread_id}/"), &params)
            .await?;
        let body: ThreadResponse = parse_json(resp).await?;
        body.thread
            .ok_or_else(|| IgError::Other(format!("missing thread {thread_id}")))
    }

    pub async fn direct_message_unsend(&self, thread_id: &str, message_id: &str) -> IgResult<bool> {
        let mut data = self.default_data(json!({}));
        if let Value::Object(map) = &mut data {
            map.remove("device_id");
            map.remove("_uid");
        }
        let resp = self
            .post_signed(
                &format!("direct_v2/threads/{thread_id}/items/{message_id}/delete/"),
                &data,
            )
            .await?;
        let body: StatusResponse = parse_json(resp).await?;
        Ok(body.status.as_deref() == Some("ok"))
    }

    pub async fn direct_reaction_remove(
        &self,
        thread_id: &str,
        message_id: &str,
        emoji: &str,
        target_item_type: Option<&str>,
        original_message_client_context: Option<&str>,
    ) -> IgResult<bool> {
        let token = Uuid::new_v4().to_string();
        let mut data = json!({
            "action": "send_item",
            "is_x_transport_forward": "false",
            "send_silently": "false",
            "is_shh_mode": "0",
            "send_attribution": "message_reaction",
            "client_context": token,
            "mutation_token": token,
            "btt_dual_send": "false",
            "is_ae_dual_send": "false",
            "offline_threading_id": token,
            "thread_ids": serde_json::to_string(&[thread_id]).unwrap_or_default(),
            "item_type": "reaction",
            "reaction_type": "like",
            "reaction_status": "deleted",
            "node_type": "item",
            "item_id": message_id,
            "emoji": emoji,
            "reaction_action_source": "double_tap",
            "nav_chain": "1qT:feed_timeline:1,1qT:feed_timeline:2,1qT:feed_timeline:3,7Az:direct_inbox:4,7Az:direct_inbox:5,5rG:direct_thread:7",
        });
        if let Some(item_type) = target_item_type {
            data["target_item_type"] = json!(item_type);
        }
        if let Some(context) = original_message_client_context.filter(|v| !v.is_empty()) {
            data["original_message_client_context"] = json!(context);
        }
        let resp = self
            .post_form(
                "direct_v2/threads/broadcast/reaction/",
                &self.default_data(data),
            )
            .await?;
        let body: StatusResponse = parse_json(resp).await?;
        Ok(body.status.as_deref() == Some("ok"))
    }

    pub async fn direct_thread_hide(&self, thread_id: &str) -> IgResult<bool> {
        let data = json!({
            "should_move_future_requests_to_spam": false,
            "_uuid": self.device.uuid,
        });
        let resp = self
            .post_form(&format!("direct_v2/threads/{thread_id}/hide/"), &data)
            .await?;
        let body: StatusResponse = parse_json(resp).await?;
        Ok(body.status.as_deref() == Some("ok"))
    }

    fn default_data(&self, extra: Value) -> Value {
        let live = self.live.lock().expect("live auth poisoned");
        let uid = live.user_id.clone();
        let mut map = serde_json::Map::new();
        map.insert("_uuid".into(), json!(self.device.uuid));
        map.insert("_uid".into(), json!(uid));
        map.insert("device_id".into(), json!(self.device.android_device_id));
        if let Some(csrf) = live.cookies.get("csrftoken") {
            map.insert("_csrftoken".into(), json!(csrf));
        }
        if let Value::Object(extra_map) = extra {
            for (k, v) in extra_map {
                map.insert(k, v);
            }
        }
        Value::Object(map)
    }

    fn action_data(&self, extra: Value) -> Value {
        self.default_data(extra_with_radio(extra))
    }

    async fn post_status(&self, path: &str, data: &Value) -> IgResult<bool> {
        let resp = self.post_signed(path, data).await?;
        let body: StatusResponse = parse_json(resp).await?;
        Ok(body.status.as_deref() == Some("ok"))
    }

    pub(crate) async fn raw_get_json(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> IgResult<Value> {
        let resp = self.get(path, params).await?;
        parse_json(resp).await
    }

    async fn get(&self, path: &str, params: &[(&str, &str)]) -> IgResult<wreq::Response> {
        self.request("GET", path, params, None).await
    }

    async fn post_signed(&self, path: &str, data: &Value) -> IgResult<wreq::Response> {
        self.request("POST", path, &[], Some((data, true))).await
    }

    async fn post_form(&self, path: &str, data: &Value) -> IgResult<wreq::Response> {
        self.request("POST", path, &[], Some((data, false))).await
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        params: &[(&str, &str)],
        body: Option<(&Value, bool)>,
    ) -> IgResult<wreq::Response> {
        for attempt in 0..8u32 {
            if self.rate_limiter.is_cancelled() {
                return Err(IgError::Cancelled);
            }
            self.rate_limiter.wait().await;
            if self.rate_limiter.is_cancelled() {
                return Err(IgError::Cancelled);
            }

            let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
            let mut builder = match method {
                "GET" => self.http.get(&url),
                "POST" => self.http.post(&url),
                other => return Err(IgError::Other(format!("unsupported method {other}"))),
            };
            builder = builder.headers(self.base_headers());
            if !params.is_empty() {
                builder = builder.query(params);
            }
            if let Some((data, signed)) = body {
                let form = if signed {
                    generate_signature(data)
                } else {
                    form_encode(data)
                };
                builder = builder
                    .header(
                        CONTENT_TYPE,
                        "application/x-www-form-urlencoded; charset=UTF-8",
                    )
                    .body(form);
            }

            let resp = match builder.send().await {
                Ok(r) => r,
                Err(e) if attempt < 2 && !self.rate_limiter.is_cancelled() => {
                    tracing::warn!("network error on {path}: {e}; retrying (attempt {attempt})");
                    tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            self.absorb_response_headers(&resp);

            let status = resp.status().as_u16();
            let retry_after_ms = parse_retry_after_ms(&resp);

            if (500..600).contains(&status) && attempt < 2 && !self.rate_limiter.is_cancelled() {
                tracing::warn!(
                    "HTTP {status} server error on {path}; retrying (attempt {attempt})"
                );
                tokio::time::sleep(Duration::from_millis(1000 * (attempt as u64 + 1))).await;
                continue;
            }
            if status == 429 {
                let wait = self
                    .rate_limiter
                    .observe_rate_limit(retry_after_ms.unwrap_or(60_000));
                tracing::warn!("HTTP 429 on {path}; cooldown {wait}ms (attempt {attempt})");
                if attempt + 1 == 8 {
                    return Err(IgError::RateLimited {
                        retry_after_ms: wait,
                        global: true,
                        scope: "user".into(),
                    });
                }
                continue;
            }

            if status == 404 {
                self.rate_limiter.observe_success();
                return Err(IgError::NotFound {
                    resource: path.to_string(),
                    status,
                });
            }

            if !(200..300).contains(&status) {
                let text = resp.text().await.unwrap_or_default();
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string));

                if let Some(msg) = &message {
                    let lower = msg.to_ascii_lowercase();
                    if lower.contains("login_required") {
                        self.rate_limiter.observe_success();
                        return Err(IgError::LoginRequired);
                    }
                    if lower.contains("challenge") {
                        self.rate_limiter.observe_success();
                        return Err(IgError::ChallengeRequired {
                            message: msg.clone(),
                        });
                    }
                    if lower.contains("feedback")
                        || lower.contains("please wait a few minutes")
                        || lower.contains("rate limit")
                    {
                        let wait = self
                            .rate_limiter
                            .observe_rate_limit(retry_after_ms.unwrap_or(120_000));
                        if attempt + 1 < 8 {
                            tracing::warn!("throttled ({msg}) on {path}; cooldown {wait}ms");
                            continue;
                        }
                        return Err(IgError::FeedbackRequired {
                            message: msg.clone(),
                        });
                    }
                }

                self.rate_limiter.observe_success();
                return match status {
                    403 => Err(IgError::Forbidden {
                        resource: path.to_string(),
                        status,
                    }),
                    401 => Err(IgError::InvalidSession { status }),
                    _ => Err(IgError::Api {
                        status,
                        message: message.unwrap_or_else(|| truncate_err(&text)),
                    }),
                };
            }

            self.rate_limiter.observe_success();

            if attempt == 0 && jitter_u64(0, 9) == 0 {
                self.persist_session();
            }
            return Ok(resp);
        }
        Err(IgError::Other("exhausted retries".into()))
    }

    fn absorb_response_headers(&self, resp: &wreq::Response) {
        let headers = resp.headers();
        let mut live = self.live.lock().expect("live auth poisoned");
        let mut dirty = false;

        for value in headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
        {
            if let Some((name, value)) = parse_set_cookie(value) {
                if value.is_empty() {
                    if live.cookies.remove(&name).is_some() {
                        dirty = true;
                    }
                    continue;
                }
                if live.cookies.get(&name) != Some(&value) {
                    live.cookies.insert(name.clone(), value.clone());
                    dirty = true;
                }
                match name.as_str() {
                    "sessionid" if value != live.sessionid => {
                        live.sessionid = value;
                        live.authorization = build_authorization(&live.user_id, &live.sessionid);
                    }
                    "ds_user_id" if value != live.user_id => {
                        live.user_id = value;
                        live.authorization = build_authorization(&live.user_id, &live.sessionid);
                    }
                    "mid" if value != live.mid => live.mid = value,
                    _ => {}
                }
            }
        }

        if let Some(v) =
            header_str(headers, "ig-set-x-mid").or_else(|| header_str(headers, "x-mid"))
        {
            if !v.is_empty() && v != live.mid {
                live.mid = v;
                let mid = live.mid.clone();
                live.cookies.insert("mid".into(), mid);
                dirty = true;
            }
        }
        if let Some(v) = header_str(headers, "ig-set-ig-u-rur")
            .or_else(|| header_str(headers, "ig-u-rur"))
            .or_else(|| header_str(headers, "x-ig-set-ig-u-rur"))
        {
            if !v.is_empty() && v != live.ig_u_rur {
                live.ig_u_rur = v;
                dirty = true;
            }
        }
        if let Some(v) = header_str(headers, "x-ig-set-www-claim")
            .or_else(|| header_str(headers, "ig-set-www-claim"))
            .or_else(|| header_str(headers, "x-ig-www-claim"))
        {
            if !v.is_empty() && v != "0" && v != live.ig_www_claim {
                live.ig_www_claim = v;
                dirty = true;
            }
        }
        if let Some(v) = header_str(headers, "ig-set-authorization") {
            if v.starts_with("Bearer ") && v != live.authorization {
                live.authorization = v;
                dirty = true;
            }
        }

        drop(live);
        if dirty {
            self.persist_session();
        }
    }

    fn persist_session(&self) {
        let live = self.live.lock().expect("live auth poisoned");
        let store = SessionStore {
            sessionid: live.sessionid.clone(),
            user_id: live.user_id.clone(),
            username: live.username.clone(),
            mid: live.mid.clone(),
            ig_u_rur: live.ig_u_rur.clone(),
            ig_www_claim: live.ig_www_claim.clone(),
            cookies: live.cookies.clone(),
            device: Some(self.device.clone()),
        };
        if let Err(e) = store.save(&self.session_path) {
            tracing::warn!(
                "failed to persist session to {}: {e}",
                self.session_path.display()
            );
        }
    }

    fn base_headers(&self) -> HeaderMap {
        let live = self.live.lock().expect("live auth poisoned");
        let mut headers = HeaderMap::new();
        let locale = &self.device.locale;
        let lang = locale.replace('_', "-");

        set(&mut headers, USER_AGENT, &self.device.user_agent);
        let accept_language = if lang == "en-US" {
            "en-US".to_string()
        } else {
            format!("{lang}, en-US")
        };
        set(&mut headers, "Accept-Language", &accept_language);
        set(&mut headers, "Accept-Encoding", "gzip, deflate");
        set(&mut headers, "X-IG-App-Locale", locale);
        set(&mut headers, "X-IG-Device-Locale", locale);
        set(&mut headers, "X-IG-Mapped-Locale", locale);
        set(
            &mut headers,
            "X-Pigeon-Session-Id",
            &format!("UFS-{}-1", self.device.client_session_id),
        );
        set(
            &mut headers,
            "X-Pigeon-Rawclienttime",
            &format!(
                "{:.3}",
                chrono::Utc::now().timestamp_millis() as f64 / 1000.0
            ),
        );

        set(&mut headers, "X-IG-Bandwidth-Speed-KBPS", "-1.000");
        set(&mut headers, "X-IG-Bandwidth-TotalBytes-B", "0");
        set(&mut headers, "X-IG-Bandwidth-TotalTime-MS", "0");
        set(
            &mut headers,
            "X-IG-App-Startup-Country",
            &self.device.country,
        );
        set(
            &mut headers,
            "X-Bloks-Version-Id",
            &self.device.bloks_versioning_id,
        );
        let claim = if live.ig_www_claim.is_empty() {
            "0"
        } else {
            live.ig_www_claim.as_str()
        };
        set(&mut headers, "X-IG-WWW-Claim", claim);
        set(&mut headers, "X-Bloks-Is-Layout-RTL", "false");
        set(&mut headers, "X-Bloks-Is-Panorama-Enabled", "true");
        set(&mut headers, "X-IG-Device-ID", &self.device.uuid);
        set(&mut headers, "X-IG-Family-Device-ID", &self.device.phone_id);
        set(
            &mut headers,
            "X-IG-Android-ID",
            &self.device.android_device_id,
        );
        set(
            &mut headers,
            "X-IG-Timezone-Offset",
            &self.device.timezone_offset.to_string(),
        );
        set(&mut headers, "X-IG-Connection-Type", "WIFI");
        set(&mut headers, "X-IG-Capabilities", "3brTv10=");
        set(&mut headers, "X-IG-App-ID", &self.device.app_id);
        set(&mut headers, "Priority", "u=3");
        set(&mut headers, "Host", API_DOMAIN);
        set(&mut headers, "X-FB-HTTP-Engine", "Tigon/MNS/TCP");
        set(&mut headers, "X-FB-Client-IP", "True");
        set(&mut headers, "X-FB-Server-Cluster", "True");
        set(&mut headers, "X-Tigon-Is-Retry", "False");
        set(&mut headers, "X-Zero-Balance", "INIT");
        set(&mut headers, "X-Zero-Eh", "");
        set(&mut headers, "X-Zero-State", "unknown");
        set(&mut headers, "Zero-HTTP-Network-Interface", "wifi");
        set(&mut headers, "Connection", "keep-alive");
        set(&mut headers, "IG-INTENDED-USER-ID", &live.user_id);
        set(&mut headers, "IG-U-DS-USER-ID", &live.user_id);
        set(
            &mut headers,
            "X-IG-Nav-Chain",
            "9MV:self_profile:2,ProfileMediaTabFragment:self_profile:3,9Xf:self_following:4",
        );
        if !live.ig_u_rur.is_empty() {
            set(&mut headers, "IG-U-RUR", &live.ig_u_rur);
        }
        set(&mut headers, "X-MID", &live.mid);
        // salt IDs are regenerated per request; only the plausible range matters
        set(
            &mut headers,
            "X-IG-SALT-IDS",
            &jitter_u64(1_061_162_222, 1_061_262_222).to_string(),
        );
        set(&mut headers, "Authorization", &live.authorization);

        let mut cookies = live.cookies.clone();
        cookies.insert("sessionid".into(), live.sessionid.clone());
        cookies.insert("ds_user_id".into(), live.user_id.clone());
        if !live.mid.is_empty() {
            cookies.insert("mid".into(), live.mid.clone());
        }
        let cookie = cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        set(&mut headers, COOKIE, &cookie);

        headers
    }
}

fn merge_object(target: &mut Value, extra: Option<Value>) {
    if let (Some(target), Some(Value::Object(extra))) = (target.as_object_mut(), extra) {
        target.extend(extra);
    }
}

impl Drop for InstagramClient {
    fn drop(&mut self) {
        self.persist_session();
    }
}

fn set(map: &mut HeaderMap, key: impl AsHeaderName, value: &str) {
    if let Ok(val) = HeaderValue::from_str(value) {
        key.insert(map, val);
    }
}

trait AsHeaderName {
    fn insert(self, map: &mut HeaderMap, val: HeaderValue);
}

impl AsHeaderName for HeaderName {
    fn insert(self, map: &mut HeaderMap, val: HeaderValue) {
        map.insert(self, val);
    }
}

impl AsHeaderName for &'static str {
    fn insert(self, map: &mut HeaderMap, val: HeaderValue) {
        if let Ok(name) = HeaderName::from_bytes(self.as_bytes()) {
            map.insert(name, val);
        }
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.as_str().eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.to_str().ok())
        .map(str::to_string)
}

fn parse_cookie_pairs(raw: &str) -> BTreeMap<String, String> {
    const SESSION_COOKIES: &[&str] = &[
        "sessionid",
        "csrftoken",
        "ds_user_id",
        "mid",
        "rur",
        "ig_did",
        "datr",
        "shbid",
        "shbts",
        "ig_nrcb",
        "wd",
    ];

    raw.trim()
        .strip_prefix("Cookie:")
        .unwrap_or(raw.trim())
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            let name = name.trim();
            let value = value.trim().trim_matches('"');
            (SESSION_COOKIES.contains(&name) && !value.is_empty())
                .then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

fn parse_set_cookie(raw: &str) -> Option<(String, String)> {
    const SESSION_COOKIES: &[&str] = &[
        "sessionid",
        "csrftoken",
        "ds_user_id",
        "mid",
        "rur",
        "ig_did",
        "datr",
        "shbid",
        "shbts",
        "ig_nrcb",
        "wd",
    ];
    let (name, value) = raw.split(';').next()?.trim().split_once('=')?;
    let name = name.trim().to_string();
    if !SESSION_COOKIES.contains(&name.as_str()) {
        return None;
    }
    let value = value.trim().trim_matches('"');
    // live.sessionid is kept decoded, so an encoded Set-Cookie must not look like a new session
    let value = if name == "sessionid" {
        decode_cookie_value(value)
    } else {
        value.to_string()
    };
    Some((name, value))
}

fn decode_cookie_value(value: &str) -> String {
    urlencoding::decode(value)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| value.to_string())
}

fn build_authorization(user_id: &str, sessionid: &str) -> String {
    let data = json!({
        "ds_user_id": user_id,
        "sessionid": sessionid,
        "should_use_header_over_cookies": true,
    });
    let b64 = BASE64.encode(serde_json::to_vec(&data).unwrap_or_default());
    format!("Bearer IGT:2:{b64}")
}

fn generate_signature(data: &Value) -> String {
    let dumped = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
    format!("signed_body=SIGNATURE.{}", form_component(&dumped))
}

fn extra_with_radio(extra: Value) -> Value {
    match extra {
        Value::Object(mut map) => {
            map.entry("radio_type")
                .or_insert_with(|| json!("wifi-none"));
            Value::Object(map)
        }
        other => other,
    }
}

fn form_encode(data: &Value) -> String {
    let mut parts = Vec::new();
    if let Value::Object(map) = data {
        for (k, v) in map {
            let val = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
                other => other.to_string().trim_matches('"').to_string(),
            };
            parts.push(format!("{}={}", form_component(k), form_component(&val)));
        }
    }
    parts.join("&")
}

fn form_component(value: &str) -> String {
    urlencoding::encode(value).replace("%20", "+")
}

fn normalize_sessionid(raw: &str) -> IgResult<String> {
    let s = raw.trim().trim_matches('"');
    let s = if let Some(idx) = s.find("sessionid=") {
        let after = &s[idx + "sessionid=".len()..];
        after.split(';').next().unwrap_or(after).trim()
    } else {
        s.split(';').next().unwrap_or(s).trim()
    };
    if s.len() < 20 {
        return Err(IgError::BadSessionId);
    }
    Ok(decode_cookie_value(s))
}

fn user_id_from_sessionid(sessionid: &str) -> IgResult<String> {
    let digits: String = sessionid
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return Err(IgError::BadSessionId);
    }
    Ok(digits)
}

fn parse_retry_after_ms(resp: &wreq::Response) -> Option<u64> {
    // Instagram sends Retry-After as fractional seconds rather than the usual integer
    resp.headers().get("retry-after").and_then(|v| {
        v.to_str()
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|secs| (secs * 1000.0) as u64)
    })
}

async fn parse_json<T: serde::de::DeserializeOwned>(resp: wreq::Response) -> IgResult<T> {
    let bytes = resp.bytes().await?;
    match serde_json::from_slice(&bytes) {
        Ok(val) => Ok(val),
        Err(e) => {
            let preview = String::from_utf8_lossy(&bytes);
            tracing::warn!(
                "JSON parse error ({e}) on response: {}",
                truncate_err(&preview)
            );
            Err(IgError::Json(e))
        }
    }
}

fn truncate_err(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.trim().chars();
    for c in chars.by_ref().take(240) {
        out.push(c);
    }
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

pub async fn list_all_threads(
    client: &InstagramClient,
    max_threads: usize,
) -> IgResult<Vec<DirectThread>> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let (page, next) = client.direct_inbox_page(cursor.as_deref(), 10).await?;
        let n = page.len();
        out.extend(page);
        if max_threads > 0 && out.len() >= max_threads {
            out.truncate(max_threads);
            break;
        }
        if next.is_none() || n == 0 || next == cursor {
            break;
        }
        cursor = next;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_cookie_sessionid_matches_the_normalized_form() {
        let raw = "99%3Atokenvalueherexxxxxx";
        let (name, value) =
            parse_set_cookie(&format!("sessionid={raw}; Path=/; HttpOnly")).unwrap();
        assert_eq!(name, "sessionid");
        assert_eq!(value, normalize_sessionid(raw).unwrap());
    }

    #[test]
    fn set_cookie_keeps_other_values_and_ignores_unknown_names() {
        let (_, value) = parse_set_cookie("rur=\"PRN%2C1\"; Path=/").unwrap();
        assert_eq!(value, "PRN%2C1");
        assert!(parse_set_cookie("other=1; Path=/").is_none());
    }

    #[test]
    fn normalize_cookie_form() {
        let s = normalize_sessionid("sessionid=99%3Atokenvalueherexxxxxx; Path=/").unwrap();
        assert!(s.starts_with("99"));
    }
    #[test]
    fn normalize_cookie_header_with_other_cookies() {
        let s = normalize_sessionid(
            "mid=XYZ; ds_user_id=123; sessionid=99%3Atokenvalueherexxxxxx; rur=PRN",
        )
        .unwrap();
        assert!(s.starts_with("99"));
    }

    #[test]
    fn user_id_prefix() {
        assert_eq!(user_id_from_sessionid("4242:abc:def").unwrap(), "4242");
    }

    #[test]
    fn signature_prefix() {
        let body = generate_signature(&json!({"_uuid": "x"}));
        assert!(body.starts_with("signed_body=SIGNATURE."));
    }

    #[test]
    fn request_bodies_keep_the_order_their_fields_were_written_in() {
        let body = generate_signature(&json!({"b": 1, "a": 2}));
        assert!(
            body.find("b").unwrap() < body.find("%22a").unwrap(),
            "{body}"
        );
    }

    #[test]
    fn authorization_header_shape() {
        let h = build_authorization("1", "1:sess");
        assert!(h.starts_with("Bearer IGT:2:"));
    }
}
