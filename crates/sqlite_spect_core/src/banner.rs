pub fn format_banner(db_ids: &[String]) -> String {
    let bar: String = "━".repeat(63);
    let db_count = db_ids.len();
    let db_word = if db_count == 1 { "database" } else { "databases" };
    let db_list = db_ids.join(", ");
    format!(
        "\n{bar}\n  sqlite_spect  ({db_count} {db_word}: {db_list})\n  Tip: run sqlite_spect attach to connect and open the inspector.\n{bar}"
    )
}

pub fn emit(db_ids: &[String]) {
    tracing::info!("{}", format_banner(db_ids));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_banner_lists_databases() {
        let out = format_banner(&["oia".into(), "analytics".into()]);
        assert!(out.contains("2 databases: oia, analytics"));
        assert!(out.contains("sqlite_spect attach"));
    }

    #[test]
    fn format_banner_singular_for_one_db() {
        let out = format_banner(&["oia".into()]);
        assert!(out.contains("1 database: oia"));
    }
}
