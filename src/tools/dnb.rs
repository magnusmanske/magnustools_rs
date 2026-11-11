//! Shared helpers of the dnbtools ports.

use crate::replicas::{ReplicasError, select_in};
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::Value;
use wikimisc::mysql_async::prelude::FromRow;

/// The database title shown by the wiki: spaces instead of underscores.
pub(super) fn display(title: &str) -> String {
    crate::replicas::display_title(title)
}

/// As [`select_in`], in smaller chunks: large `IN` lists make the replica's
/// optimizer choose slow plans on the joins these tools use.
pub(super) async fn select_small<T, V>(conn: &mut Conn, sql: &str, values: &[V]) -> Result<Vec<T>, ReplicasError>
where
    T: FromRow + Send + 'static,
    V: Into<Value> + Clone,
{
    let mut rows = vec![];
    for chunk in values.chunks(1_000) {
        rows.extend(select_in(conn, sql, "", &[], chunk).await?);
    }
    Ok(rows)
}
