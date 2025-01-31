use std::collections::HashMap;
use std::iter::Iterator;
use std::process::Command;

use anyhow::Error;
use clap::{Parser, Subcommand};
use colored::*;
use comfy_table::{presets, Cell, CellAlignment, Table};
use dialoguer::Confirm;
use regex::Regex;

type GEResult<T> = Result<T, Error>;

fn run_git(cmdargs: Vec<&str>, verbose: bool) -> GEResult<String> {
    let cmd_string = format!("{} {}", "git".bright_white().on_green(), cmdargs.join(" "));

    if verbose {
        println!("{}", cmd_string);
    }
    let output = Command::new("git").args(cmdargs).output()?;
    if !output.status.success() {
        println!("{}", String::from_utf8(output.stderr)?);
        return Err(Error::msg(format!(
            "git exited with status {}",
            output.status.code().unwrap_or(-1)
        )));
    }
    let output = String::from_utf8(output.stdout)?;
    let trimmed = output.trim();
    if verbose {
        println!("{}", trimmed)
    }

    Ok(String::from(trimmed))
}

fn lasthash(verbose: bool) -> GEResult<String> {
    run_git(vec!["log", "-n", "1", "--pretty=format:%H"], verbose)
}

fn ensure_clean() -> GEResult<()> {
    let status = run_git(vec!["status"], false)?;
    if !(status.contains("nothing to commit, working directory clean")
        || status.contains("nothing to commit, working tree clean"))
    {
        return Err(Error::msg(status.white().on_bright_red()));
    }
    Ok(())
}

fn handle_submodules(verbose: bool) -> GEResult<()> {
    run_git(vec!["submodule", "init"], verbose)?;
    run_git(vec!["submodule", "update", "--recursive"], verbose)?;
    Ok(())
}

fn get_upstream(verbose: bool) -> GEResult<String> {
    run_git(
        vec!["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        verbose,
    )
}

fn get_curr_branch(verbose: bool) -> GEResult<String> {
    run_git(vec!["rev-parse", "--abbrev-ref", "HEAD"], verbose)
}

fn fix_upstream(upstream: &str, verbose: bool) -> GEResult<()> {
    let commit = lasthash(verbose)?;
    run_git(vec!["branch", "--set-upstream-to", upstream], true)?;
    ensure_clean()?;
    run_git(vec!["reset", "--hard", upstream, "--"], true)?;
    handle_submodules(true)?;
    run_git(vec!["cherry-pick", commit.as_str()], true)?;
    handle_submodules(true)?;
    Ok(())
}

fn checkout(branch: &str, verbose: bool) -> GEResult<()> {
    run_git(vec!["checkout", branch], verbose)?;
    handle_submodules(verbose)
}

fn rec_fix_up(
    terminal: &str,
    push: bool,
    verbose: bool,
    branch_cache: &mut Vec<String>,
) -> GEResult<()> {
    let curr_branch = get_curr_branch(verbose)?;
    if curr_branch == terminal {
        for branch in branch_cache {
            checkout(branch, true)?;
            fix_upstream(&get_upstream(false)?, verbose)?;
            if push {
                push_origin(false)?;
            }
        }
        return Ok(());
    }
    let curr_upstream = get_upstream(verbose)?;
    checkout(&curr_upstream, false)?;
    branch_cache.insert(0, curr_branch);
    rec_fix_up(terminal, push, verbose, branch_cache)
}

fn commit_branch(branch_name: &str, verbose: bool) -> GEResult<()> {
    run_git(vec!["branch", branch_name], true)?;
    ensure_clean()?;
    run_git(vec!["reset", "--hard", "HEAD~1"], true)?;
    let parent_branch = get_curr_branch(verbose)?;
    run_git(vec!["checkout", branch_name], true)?;
    run_git(vec!["branch", "--set-upstream-to", &parent_branch], true)?;
    handle_submodules(true)
}

fn push_origin(verbose: bool) -> GEResult<()> {
    let branch = get_curr_branch(verbose)?;
    run_git(vec!["push", "-f", "origin", &branch], true)?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct Status {
    ahead: Option<i32>,
    behind: Option<i32>,
}

impl Status {
    fn parse(s: &str) -> Option<Status> {
        let parser = Regex::new(r"(?:ahead (\d+))?(?:, )?(?:behind (\d+))?").unwrap();
        if let Some(caps) = parser.captures(s) {
            Some(Status {
                ahead: caps.get(1).and_then(|it| it.as_str().parse().ok()),
                behind: caps.get(2).and_then(|it| it.as_str().parse().ok()),
            })
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
struct BranchDescriptor {
    current: bool,
    name: String,
    sha: String,
    upstream: Option<String>,
    message: String,
    status: Option<Status>,
}

#[derive(Clone, Debug)]
struct BranchT {
    desc: BranchDescriptor,
    downstream: Vec<String>,
}

impl BranchT {
    fn has_upstream(&self) -> bool {
        self.desc.upstream.is_some()
    }
}

fn branch_depth(branches_by_name: &HashMap<String, BranchT>, branch_name: &str) -> i32 {
    if let Some(br) = branches_by_name.get(branch_name) {
        if let Some(up) = &br.desc.upstream {
            1 + branch_depth(branches_by_name, up)
        } else {
            0
        }
    } else {
        0
    }
}

fn parse_error(branch_entry: &str, reason: &str) -> Error {
    Error::msg(format!(
        "Unexpectedly unable to parse branch line {} ({})",
        branch_entry, reason
    ))
}

fn parse_branch_entry(branch_entry: &str) -> GEResult<BranchDescriptor> {
    let whitespace = Regex::new(r"\s+")?;
    let parts: Vec<&str> = whitespace
        .splitn(branch_entry.trim().trim_start_matches('*').trim(), 3)
        .collect();
    if parts.len() != 3 {
        return Err(parse_error(branch_entry, "wrong number of parts"));
    }
    let rest = parts[2];
    let rest_expr = Regex::new(r"(?:\[([^\]]*)\] )?(.*)")?;
    let group = rest_expr
        .captures(rest)
        .ok_or_else(|| parse_error(branch_entry, "failed to capture"))?;

    let upstream_and_maybe_status: Option<Vec<&str>> =
        group.get(1).map(|s| s.as_str().split(": ").collect());

    let upstream = upstream_and_maybe_status
        .clone()
        .map(|v| String::from(v[0]));

    let status = upstream_and_maybe_status
        .and_then(|v| v.get(1).cloned())
        .and_then(|it| Status::parse(it));

    let descriptor = BranchDescriptor {
        current: branch_entry.chars().next().unwrap_or(' ') == '*',
        name: String::from(parts[0]),
        sha: String::from(parts[1]),
        message: String::from(
            group
                .get(2)
                .ok_or_else(|| parse_error(branch_entry, "no message"))?
                .as_str(),
        ),
        upstream,
        status,
    };

    Ok(descriptor)
}

const INDENT_AMOUNT: i32 = 2;

fn prefix_for_depth(depth: i32) -> String {
    if depth <= 0 {
        String::from("")
    } else {
        " ".repeat((INDENT_AMOUNT * depth) as usize) + "+-- "
    }
}

fn format_tree_rooted_at(
    branches_by_name: &HashMap<String, BranchT>,
    root: &BranchT,
) -> GEResult<Vec<Vec<Cell>>> {
    let depth = branch_depth(branches_by_name, &root.desc.name);
    let prefix = prefix_for_depth(depth) + if root.desc.current { "* " } else { "" };
    let upstream_prefix = prefix_for_depth(depth - 1);

    let mut output_rows = if let Some(up) = &root.desc.upstream {
        if up.contains("origin") {
            vec![vec![
                Cell::new(upstream_prefix + up).fg(comfy_table::Color::DarkBlue),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]]
        } else if !branches_by_name.contains_key(up) {
            vec![vec![
                Cell::new(upstream_prefix + up + " [missing]").fg(comfy_table::Color::Red),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]]
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    output_rows.push(vec![
        Cell::new(prefix + &root.desc.name),
        Cell::new(root.desc.sha.clone()),
        Cell::new(
            root.desc
                .status
                .and_then(|it| it.ahead)
                .map(|it| format!("+{it}"))
                .unwrap_or("".to_string()),
        )
        .fg(comfy_table::Color::DarkGreen),
        Cell::new(
            root.desc
                .status
                .and_then(|it| it.behind)
                .map(|it| format!("-{it}"))
                .unwrap_or("".to_string()),
        )
        .fg(comfy_table::Color::Red),
        if root.desc.current {
            Cell::new(root.desc.message.clone()).fg(comfy_table::Color::DarkGreen)
        } else {
            Cell::new(root.desc.message.clone())
        },
    ]);
    for down_name in &root.downstream {
        if let Some(down) = branches_by_name.get(down_name) {
            output_rows.append(&mut format_tree_rooted_at(branches_by_name, down)?)
        }
    }
    Ok(output_rows)
}

fn print_branch_tree() -> GEResult<()> {
    let branch_names: Vec<String> = run_git(vec!["branch", "-vv"], false)?
        .lines()
        .map(String::from)
        .collect();
    let mut branch_downstream_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut branches: Vec<BranchT> = vec![];
    for branch in &branch_names {
        let desc = parse_branch_entry(branch)?;
        branches.push(BranchT {
            desc,
            downstream: vec![],
        });
    }

    for branch in &branches {
        if let Some(upstream) = &branch.desc.upstream {
            if !branch_downstream_map.contains_key(upstream) {
                branch_downstream_map.insert(upstream.clone(), vec![]);
            }
            branch_downstream_map
                .get_mut(upstream)
                .ok_or_else(|| Error::msg("Upstream branch missing!"))?
                .push(branch.desc.name.clone());
        }
    }

    for branch in branches.iter_mut() {
        if let Some(downstream) = branch_downstream_map.get(&branch.desc.name) {
            branch.downstream = downstream.to_vec();
        }
    }

    let mut branches_by_name: HashMap<String, BranchT> = HashMap::new();
    for branch in &branches {
        branches_by_name.insert(branch.desc.name.clone(), branch.clone());
    }

    let mut root_branches: Vec<BranchT> = branches
        .into_iter()
        .filter(|b| {
            !b.has_upstream()
                || !branches_by_name
                    .contains_key(b.desc.upstream.as_ref().unwrap_or(&String::from("")))
        })
        .collect();
    root_branches.sort_by_key(|br| br.desc.name.clone());

    let mut all_rows: Vec<Vec<Cell>> = vec![];

    for br in root_branches {
        all_rows.append(&mut format_tree_rooted_at(&branches_by_name, &br)?)
    }

    let mut table = Table::new();
    table.load_preset(presets::NOTHING);
    for row in all_rows {
        table.add_row(row);
    }
    table.set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    table
        .get_column_mut(0)
        .unwrap()
        .set_cell_alignment(CellAlignment::Left);
    let col1 = table.get_column_mut(1).unwrap();
    col1.set_cell_alignment(CellAlignment::Right);
    col1.set_padding((0, 0));
    table
        .get_column_mut(2)
        .unwrap()
        .set_cell_alignment(CellAlignment::Right);
    let col3 = table.get_column_mut(3).unwrap();
    col3.set_cell_alignment(CellAlignment::Right);
    col3.set_padding((0, 0));
    table
        .get_column_mut(4)
        .unwrap()
        .set_cell_alignment(CellAlignment::Left);
    println!("{table}");

    Ok(())
}

fn delete_branch(branch: &str, verbose: bool) -> GEResult<()> {
    run_git(vec!["branch", "-D", branch], verbose)?;
    Ok(())
}

fn purge(prefix: &str, no_confirm: bool, verbose: bool) -> GEResult<()> {
    let re = Regex::new(&format!(r"origin/{}/([\w-]+)", prefix))?;
    let branches: std::vec::Vec<String> =
        run_git(vec!["remote", "prune", "origin", "-n"], verbose)?
            .lines()
            .map(|s| s.trim())
            .map(|s| re.captures(s))
            .flatten()
            .map(|cap| cap.get(1))
            .flatten()
            .map(|m| format!("{}/{}", prefix, m.as_str()))
            .collect();
    if branches.is_empty() {
        println!("No branches to purge.");
        return Ok(());
    }
    println!("I'm going to purge the following branches:");
    for branch in &branches {
        println!("{}", branch);
    }
    if no_confirm {
        for branch in &branches {
            let result = delete_branch(branch, true);
            if let Err(e) = result {
                println!("Warning: ignoring error deleting branch {}: {}", branch, e)
            }
        }
        run_git(vec!["remote", "prune", "origin"], verbose)?;
    } else if Confirm::new().with_prompt("Ok?").interact()? {
        for branch in branches {
            let result = delete_branch(&branch, true);
            if let Err(e) = result {
                println!("Warning: ignoring error deleting branch {}: {}", branch, e)
            }
        }
        run_git(vec!["remote", "prune", "origin"], verbose)?;
    } else {
        println!("Cancelling.")
    }

    Ok(())
}

fn add_amend_push_origin(verbose: bool) -> GEResult<()> {
    run_git(vec!["add", "."], true)?;
    run_git(vec!["commit", "--amend", "--no-edit"], true)?;
    push_origin(verbose)
}

fn rebase_onto_latest(branch: &str, verbose: bool) -> GEResult<()> {
    let curr = get_curr_branch(false)?;
    run_git(vec!["checkout", branch], true)?;
    run_git(vec!["pull", "--ff-only"], true)?;
    run_git(vec!["checkout", &curr], true)?;
    fix_upstream(branch, verbose)
}

fn reset_hard_origin(verbose: bool) -> GEResult<()> {
    let curr = get_curr_branch(verbose)?;
    ensure_clean()?;
    run_git(vec!["fetch", "origin"], true)?;
    run_git(vec!["reset", "--hard", &format!("origin/{curr}")], true)?;
    Ok(())
}

#[derive(Debug, Subcommand)]
pub enum SubCommand {
    /// (alias: lh) print the most recent commit hash
    #[clap(alias = "lh")]
    Lasthash {},

    /// (alias: shup) print the current branch's upstream
    #[clap(alias = "shup")]
    ShowUp {},

    /// (alias: fu) rebase the latest commit onto the upstream
    #[clap(alias = "fu")]
    FixUp {},

    /// rebase the latest commit onto the specified branch
    Up { branch: String },

    /// (alias: rup) recursively rebase the latest commit onto the upstream, to the provided terminal branch
    #[clap(alias = "rup")]
    RecFixUp {
        terminal: String,
        #[clap(long)]
        push: bool,
    },

    /// (alias: cbr) reset to HEAD~1 and then create a new branch from the (formerly) current commit only
    #[clap(alias = "cbr")]
    CommitBr { name: String },

    /// (alias: tree) show the tree of all branches and their upstream relations
    #[clap(alias = "tree")]
    ShowTree {},

    /// (alias: po) force push to the same-named branch on the origin
    #[clap(alias = "po")]
    PushOrigin {},

    /// delete all branches with the given prefix that are no longer on the origin
    Purge {
        prefix: String,
        #[clap(short = 'y')]
        no_confirm: bool,
    },

    /// (alias: aap) `git add .`; `git commit --amend`; `git_ext po`
    #[clap(alias = "aap")]
    AddAmendPushOrigin {},

    /// (alias: rl) pull the latest main (or specified branch), then set the current branch to be the current commit rebased on that
    #[clap(alias = "rl")]
    RebaseOntoLatest { branch: Option<String> },

    /// (alias: rho) reset --hard to the same-named branch on the origin
    #[clap(alias = "rho")]
    ResetHardOrgin {},
}

#[derive(Debug, Parser)]
pub struct GitExt {
    #[clap(short, long)]
    verbose: bool,
    #[clap(subcommand)]
    cmd: SubCommand,
}

fn main() {
    let opt = GitExt::parse();
    use SubCommand::*;
    let verbose = opt.verbose;
    let result = match opt.cmd {
        Lasthash {} => lasthash(verbose).map(|res| {
            println!("{}", res);
        }),
        ShowUp {} => get_upstream(verbose).map(|res| {
            println!("{}", res);
        }),
        FixUp {} => fix_upstream(&get_upstream(verbose).unwrap(), verbose),
        Up { branch } => fix_upstream(&branch, verbose),
        RecFixUp { terminal, push } => rec_fix_up(&terminal, push, verbose, &mut vec![]),
        CommitBr { name } => commit_branch(&name, verbose),
        PushOrigin {} => push_origin(verbose),
        ShowTree {} => print_branch_tree(),
        Purge { prefix, no_confirm } => purge(&prefix, no_confirm, verbose),
        AddAmendPushOrigin {} => add_amend_push_origin(verbose),
        RebaseOntoLatest { branch } => {
            rebase_onto_latest(&branch.unwrap_or("main".to_string()), verbose)
        }
        ResetHardOrgin {} => reset_hard_origin(verbose),
    };
    if result.is_err() {
        eprintln!("{}", result.unwrap_err());
        std::process::exit(1)
    }
}
