//! Discord の収益化機能を操作する REST ヘルパーです。

use serde::Serialize;
use twilight_http::{
    Client, Error as HttpError, request::Request, response::DeserializeBodyError, routing::Route,
};
use twilight_model::{
    application::monetization::Entitlement,
    id::{
        Id,
        marker::{ApplicationMarker, GenericMarker, SkuMarker},
    },
};

pub use twilight_http::request::application::monetization::CreateTestEntitlementOwner;

#[derive(Serialize)]
struct CreateBody {
    sku_id: Id<SkuMarker>,
    owner_id: Id<GenericMarker>,
    owner_type: u8,
}

/// 指定したユーザーまたはサーバーにテスト用 Entitlement を作成します。
///
/// Discord の仕様どおり、SKU ID と所有者 ID を JSON の文字列で送信します。
/// 戻り値の有効期間や所有者のフィールドは、Discord の部分応答では省略されることがあります。
///
/// # エラー
///
/// リクエストや応答の読み取りに失敗した場合、または戻されたアプリ・SKU・所有者が
/// 指定と一致しない場合にエラーを返します。
pub async fn create_test_entitlement(
    http: &Client,
    application: Id<ApplicationMarker>,
    sku: Id<SkuMarker>,
    owner: CreateTestEntitlementOwner,
) -> Result<Entitlement, CreateTestEntitlementError> {
    let owner_id = match owner {
        CreateTestEntitlementOwner::Guild(id) => id.cast(),
        CreateTestEntitlementOwner::User(id) => id.cast(),
    };
    let request = Request::builder(&Route::CreateTestEntitlement {
        application_id: application.get(),
    })
    .json(&CreateBody {
        sku_id: sku,
        owner_id,
        owner_type: owner.kind(),
    })
    .build()?;
    let entitlement = http.request::<Entitlement>(request).await?.model().await?;
    let owner_matches = match owner {
        CreateTestEntitlementOwner::Guild(id) => {
            entitlement.guild_id.is_none_or(|actual| actual == id)
        }
        CreateTestEntitlementOwner::User(id) => {
            entitlement.user_id.is_none_or(|actual| actual == id)
        }
    };
    if entitlement.application_id != application || entitlement.sku_id != sku || !owner_matches {
        return Err(CreateTestEntitlementError::MismatchedResponse);
    }
    Ok(entitlement)
}

/// テスト用 Entitlement を作成できなかった理由です。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CreateTestEntitlementError {
    /// Discord へのリクエストが失敗しました。
    #[error("テスト用 Entitlement を作成できませんでした")]
    Http(#[from] HttpError),
    /// Discord の応答を Entitlement として読み取れませんでした。
    #[error("テスト用 Entitlement の応答を読み取れませんでした")]
    HttpBody(#[from] DeserializeBodyError),
    /// 応答のアプリ ID、SKU ID、または所有者 ID が指定と一致しません。
    #[error("テスト用 Entitlement の応答が要求と一致しません")]
    MismatchedResponse,
}

#[cfg(test)]
mod tests;
