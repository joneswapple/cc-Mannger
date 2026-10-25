//! Adding and refreshing upstream accounts at the `Core` level: shared by
//! the OAuth login, credential imports and the admin "refresh" action.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::accounts::{Account, Credentials, STATUS_ACTIVE};
use crate::login::{LoginIntent, Profile};
use crate::oauth;
use crate::Core;

impl Core {
    /// Stores a token set as an account. When the profile's email matches an
    /// existing account, that account gets the new tokens (a re-login clears
    /// `needs_reauth`); otherwise a new account is created.
    pub async fn upsert_account(
        &self,
        creds: Credentials,
        profile: Option<Profile>,
        intent: LoginIntent,
    ) -> Result<Account> {
        // The token response names the account but not the plan; ask the
        // profile endpoint for whatever is missing.
        let given = profile.unwrap_or_default();
        let profile = if given.email.is_empty() || given.plan.is_empty() {
            match self.login.profile(&creds.access_token).await {
                Ok(p) => Profile {
                    email: if given.email.is_empty() {
                        p.email
                    } else {
                        given.email
                    },
                    org_uuid: if given.org_uuid.is_empty() {
                        p.org_uuid
                    } else {
                        given.org_uuid
                    },
                    plan: p.plan,
                },
                Err(e) => {
                    tracing::warn!("account profile lookup failed: {e:#}");
                    given
                }
            }
        } else {
            given
        };
        let id = match self.accounts.find_by_email(&profile.email)? {
            Some(existing) => {
                self.accounts.save_credentials(existing.id, &creds)?;
                self.accounts.set_status(existing.id, STATUS_ACTIVE, "")?;
                self.pool.forget(existing.id);
                existing.id
            }
            None => {
                let label = [intent.label.trim(), profile.email.as_str()]
                    .into_iter()
                    .find(|s| !s.is_empty())
                    .unwrap_or("claude-account")
                    .to_string();
                self.accounts.create(
                    Account {
                        label,
                        enabled: true,
                        account_group: intent.account_group.trim().to_string(),
                        ..Default::default()
                    },
                    &creds,
                )?
            }
        };
        self.accounts
            .set_profile(id, &profile.email, &profile.org_uuid, &profile.plan)?;
        self.accounts.get(id)
    }

    /// Imports one credentials document in any accepted shape (see
    /// [`oauth::parse_credentials`]).
    pub async fn import_credentials(&self, doc: &Value, intent: LoginIntent) -> Result<Account> {
        let creds = oauth::parse_credentials(&serde_json::to_vec(doc)?)?;
        self.upsert_account(creds, None, intent).await
    }

    /// Imports a batch: a single document, or an array whose items are
    /// either documents or `{"label", "account_group", "credentials"}`.
    /// Returns per-item results so one bad entry doesn't sink the rest.
    pub async fn import_batch(
        &self,
        input: &Value,
        defaults: LoginIntent,
    ) -> Vec<Result<Account, String>> {
        let items: Vec<&Value> = match input {
            Value::Array(items) => items.iter().collect(),
            single => vec![single],
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let (doc, intent) = match item.get("credentials") {
                Some(doc) => (
                    doc,
                    LoginIntent {
                        label: item["label"]
                            .as_str()
                            .unwrap_or(&defaults.label)
                            .to_string(),
                        account_group: item["account_group"]
                            .as_str()
                            .unwrap_or(&defaults.account_group)
                            .to_string(),
                    },
                ),
                None => (item, defaults.clone()),
            };
            out.push(
                self.import_credentials(doc, intent)
                    .await
                    .map_err(|e| format!("{e:#}")),
            );
        }
        out
    }

    /// Refreshes an account's token now and re-reads its profile.
    pub async fn refresh_account(&self, id: i64) -> Result<Account> {
        let provider = self.pool.provider_for(id)?;
        provider.force_refresh().await?;
        let token = provider.token().await.context("token after refresh")?;
        match self.login.profile(&token).await {
            Ok(p) => self
                .accounts
                .set_profile(id, &p.email, &p.org_uuid, &p.plan)?,
            Err(e) => tracing::warn!(account = id, "profile lookup failed: {e:#}"),
        }
        self.accounts.get(id)
    }

    /// Finishes an OAuth login and stores the account.
    pub async fn complete_login(&self, code: &str, state: Option<&str>) -> Result<Account> {
        let (creds, profile, intent) = self.login.complete(code, state).await?;
        if creds.refresh_token.is_empty() {
            tracing::warn!("login returned no refresh token; the account will need a new login when it expires");
        }
        self.upsert_account(creds, Some(profile), intent).await
    }
}
