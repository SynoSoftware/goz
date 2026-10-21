//! The request gozler sends for a command line.

use super::{EsPlan, ScopeFolder};
use crate::proto::{Returns, Rows, SearchRequest};
use crate::query::{QueryError, QueryStart, TermMatch};
use crate::search::{Search, SearchInput, TextMatch};
use crate::types::EsColumn;
use crate::winpath::{absolute, parent_of};

/// The request for `plan`: its query parsed, its folders made absolute
/// against `working_dir`, an absolute path such as `C:\repo`, its `dm:`
/// values resolved against `query_start`, and either the rows to show or,
/// for a count, the totals.
///
/// The folders of `-path`, `-parent-path` and `-parent`, and the relative
/// paths in the query, are joined to `working_dir` by
/// [`absolute`]. A drive-relative folder such as
/// `D:x` depends on the current folder of each drive, which only Windows
/// knows, so the caller resolves it first (see
/// [`is_drive_relative`](crate::winpath::is_drive_relative)).
pub fn build_search(
    plan: &EsPlan,
    working_dir: &str,
    query_start: &QueryStart,
) -> Result<SearchRequest, QueryError> {
    let returns = match plan.count {
        Some(_) => Returns::Totals,
        None => Returns::Rows(Rows {
            sort: plan.sort,
            offset: 0,
            limit: plan.row_limit(),
            // Always fetch the size and date; output decides what to show.
            // Attributes and the date created travel only for their column.
            want_size: true,
            want_mtime: true,
            want_created: plan.columns.contains(&EsColumn::DateCreated),
            want_attributes: plan.columns.contains(&EsColumn::Attributes),
            want_sums: false,
        }),
    };
    let scope = plan.scope.as_ref().map(|scope| match scope {
        ScopeFolder::Dir(dir) => absolute(dir, working_dir),
        ScopeFolder::ParentOf(path) => parent_of(&absolute(path, working_dir)),
    });
    let parent = plan.parent.as_ref().map(|dir| absolute(dir, working_dir));
    let search = Search::new(&SearchInput {
        text: &plan.query,
        text_match: TextMatch::Terms(if plan.whole_word {
            TermMatch::WholeWords
        } else {
            TermMatch::Anywhere
        }),
        match_case: plan.match_case,
        match_path: plan.match_path,
        filters: &[],
        regexes: &plan.regexes,
        scope: scope.as_deref(),
        parent: parent.as_deref(),
        working_dir: Some(working_dir),
        query_start,
    })?;
    Ok(search.request(returns))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escompat::{Command, parse_argv};
    use crate::query::{Clause, Test};
    use crate::winpath::is_drive_relative;

    /// The request for `plan` in `working_dir`, at an arbitrary query start.
    fn search_in(plan: &EsPlan, working_dir: &str) -> Result<SearchRequest, QueryError> {
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        build_search(plan, working_dir, &start)
    }

    fn plan(tokens: &[&str]) -> EsPlan {
        let args: Vec<String> = tokens.iter().map(ToString::to_string).collect();
        match parse_argv(&args) {
            Ok(Command::Run(plan)) => plan,
            other => panic!("{tokens:?}: {other:?}"),
        }
    }

    /// The daemon looks each component of a scope up as a child folder and
    /// routes the scope by its drive, so every form a user types must arrive
    /// absolute, without `.` or `..`.
    #[test]
    fn a_scope_is_made_absolute_with_windows_rules() {
        let cases = [
            (r"C:\repo\docs\", r"D:\other", r"C:\repo\docs"),
            ("C:/repo/./src/../docs", r"D:\other", r"C:\repo\docs"),
            (".", r"C:\repo\src", r"C:\repo\src"),
            (r"..\docs", r"C:\repo\src", r"C:\repo\docs"),
            (r"\tmp", r"C:\repo\src", r"C:\tmp"),
            (r"C:\..\..", r"D:\other", r"C:\"),
            (r"\\server\share\a\..\b", r"C:\repo", r"\\server\share\b"),
            (r"\x", r"\\server\share\dir", r"\\server\share\x"),
            (r"\\?\Volume{1}\a\..", r"C:\repo", r"\\?\Volume{1}\a\.."),
            ("c:x", r"C:\repo", r"C:\repo\x"),
            ("D:x", r"C:\repo", r"D:\x"),
        ];
        for (scope, working_dir, absolute) in cases {
            let request = search_in(&plan(&["-path", scope]), working_dir).expect("parses");
            assert_eq!(
                request.scope.as_deref(),
                Some(absolute),
                "{scope} in {working_dir}"
            );
        }
        assert!(is_drive_relative("D:x") && is_drive_relative("D:"));
        assert!(!is_drive_relative(r"D:\x") && !is_drive_relative("x"));
    }

    /// `-parent-path` searches below the folder that holds its path, and a
    /// root holds itself. `-parent` adds its test and is the scope only when
    /// no other switch names one, so `-path` still decides the volumes.
    #[test]
    fn the_parent_switches_name_their_folders() {
        let search = |tokens: &[&str]| search_in(&plan(tokens), r"C:\repo\src").expect("parses");
        let scope = |tokens: &[&str]| search(tokens).scope;
        assert_eq!(
            scope(&["-parent-path", "main.rs"]).as_deref(),
            Some(r"C:\repo\src")
        );
        assert_eq!(scope(&["-parent-path", "C:/"]).as_deref(), Some(r"C:\"));

        let parent = search(&["-parent", ".."]);
        assert_eq!(parent.scope.as_deref(), Some(r"C:\repo"));
        assert_eq!(
            parent.query.clauses,
            [Clause::single(Test::Parent(r"C:\repo".to_string()))]
        );
        assert_eq!(
            scope(&["-parent", "..", "-path", r"D:\x"]).as_deref(),
            Some(r"D:\x")
        );
    }

    /// `-r` takes its whole argument as the pattern, which a `regex:` term
    /// would need quoted, and reads it as `regex:` does, -p included; `-ww`
    /// makes the query's text terms whole words.
    #[test]
    fn the_matching_switches_shape_the_query() {
        let tests = |tokens: &[&str]| -> Vec<Test> {
            search_in(&plan(tokens), r"C:\repo")
                .expect("parses")
                .query
                .clauses
                .into_iter()
                .flat_map(|clause| clause.terms)
                .map(|term| term.test)
                .collect()
        };
        assert_eq!(
            tests(&["-r", r#"^"a b"|c$"#, "-r", r"x\\y"]),
            [
                Test::NameRegex(r#"^"a b"|c$"#.to_string()),
                Test::PathRegex(r"x\\y".to_string()),
            ]
        );
        assert_eq!(
            tests(&["-p", "-r", "^C:"]),
            [Test::PathRegex("^C:".to_string())]
        );
        assert_eq!(
            tests(&["-ww", "report"]),
            [Test::NameWholeWord("report".to_string())]
        );
    }

    /// A count asks for the totals, which cover every match, so `-n` never
    /// reaches it; rows carry the limit.
    #[test]
    fn a_count_asks_for_the_totals_and_rows_carry_the_limit() {
        let returns = |tokens: &[&str]| search_in(&plan(tokens), r"C:\").expect("parses").returns;
        assert_eq!(returns(&["-get-result-count", "-n", "5"]), Returns::Totals);
        assert_eq!(returns(&["-get-folder-size", r"C:\x"]), Returns::Totals);
        assert!(matches!(
            returns(&["-n", "5"]),
            Returns::Rows(Rows { limit: Some(5), .. })
        ));
    }
}
