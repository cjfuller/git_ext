use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt::Display;
use std::iter::Iterator;
use std::process::Command;
use std::rc::Rc;

use anyhow::Error;
use clap::{Parser, Subcommand};
use colored::*;
use comfy_table::{Cell, CellAlignment, Table, presets};
use dialoguer::Confirm;
use git_ext::tree::{Tree, draw_from};
use indexmap::IndexMap;
use regex::Regex;

type GEResult<T> = Result<T, Error>;

fn run_git(cmdargs: &[&str], verbose: bool) -> GEResult<String> {
    let cmd_string = format!("{} {}", "git".bright_black().on_green(), cmdargs.join(" "));

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
    run_git(&["log", "-n", "1", "--pretty=format:%H"], verbose)
}

fn ensure_clean() -> GEResult<()> {
    let status = run_git(&["status"], false)?;
    if !(status.contains("nothing to commit, working directory clean")
        || status.contains("nothing to commit, working tree clean"))
    {
        return Err(Error::msg(status.white().on_bright_red()));
    }
    Ok(())
}

fn handle_submodules(verbose: bool) -> GEResult<()> {
    run_git(&["submodule", "init"], verbose)?;
    run_git(&["submodule", "update", "--recursive"], verbose)?;
    Ok(())
}

fn get_upstream(verbose: bool) -> GEResult<String> {
    run_git(
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        verbose,
    )
}

fn get_curr_branch(verbose: bool) -> GEResult<String> {
    run_git(&["rev-parse", "--abbrev-ref", "HEAD"], verbose)
}

fn fix_upstream(upstream: &str, verbose: bool) -> GEResult<()> {
    let commit = lasthash(verbose)?;
    run_git(&["branch", "--set-upstream-to", upstream], true)?;
    ensure_clean()?;
    run_git(&["reset", "--hard", upstream, "--"], true)?;
    handle_submodules(true)?;
    run_git(&["cherry-pick", commit.as_str()], true)?;
    handle_submodules(true)?;
    Ok(())
}

fn checkout(branch: &str, verbose: bool) -> GEResult<()> {
    run_git(&["checkout", branch], verbose)?;
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
    run_git(&["branch", branch_name], true)?;
    ensure_clean()?;
    run_git(&["reset", "--hard", "HEAD~1"], true)?;
    let parent_branch = get_curr_branch(verbose)?;
    run_git(&["checkout", branch_name], true)?;
    run_git(&["branch", "--set-upstream-to", &parent_branch], true)?;
    handle_submodules(true)
}

fn push_origin(verbose: bool) -> GEResult<()> {
    let branch = get_curr_branch(verbose)?;
    run_git(&["push", "-f", "origin", &branch], true)?;
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
        parser.captures(s).map(|caps| Status {
            ahead: caps.get(1).and_then(|it| it.as_str().parse().ok()),
            behind: caps.get(2).and_then(|it| it.as_str().parse().ok()),
        })
    }
}

#[derive(Clone, Debug)]
enum Branch {
    Branch {
        current: bool,
        name: String,
        sha: String,
        upstream: Option<String>,
        message: String,
        status: Option<Status>,
    },
    Missing(String),
}

impl Branch {
    fn name(&self) -> &str {
        match self {
            Branch::Branch { name, .. } => name,
            Branch::Missing(name) => name,
        }
    }
    fn upstream(&self) -> Option<&str> {
        match self {
            Branch::Branch { upstream, .. } => upstream.as_deref(),
            Branch::Missing(_) => None,
        }
    }
    fn has_upstream(&self) -> bool {
        self.upstream().is_some()
    }
}

impl Display for Branch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Branch::Branch { current, name, .. } if *current => write!(f, "* {name}"),
            Branch::Missing(name) if !name.contains("origin") => write!(f, "{name} [missing]"),
            _ => write!(f, "{}", self.name()),
        }
    }
}

fn parse_error(branch_entry: &str, reason: &str) -> Error {
    Error::msg(format!(
        "Unexpectedly unable to parse branch line {} ({})",
        branch_entry, reason
    ))
}

fn parse_branch_entry(branch_entry: &str) -> GEResult<Branch> {
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
        .and_then(Status::parse);

    let descriptor = Branch::Branch {
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

fn build_branch_trees() -> GEResult<Vec<Rc<RefCell<Tree<Branch>>>>> {
    let branch_names: Vec<String> = run_git(&["branch", "-vv"], false)?
        .lines()
        .map(String::from)
        .collect();
    let mut branches: IndexMap<String, Branch> = Default::default();
    let mut downstream_lookup: IndexMap<String, Vec<String>> = Default::default();
    for branch in &branch_names {
        let desc = parse_branch_entry(branch)?;
        branches.insert(desc.name().to_string(), desc.clone());
    }

    for branch in branches.values().cloned().collect::<Vec<_>>() {
        if let Some(up_name) = branch.upstream() {
            branches
                .entry(up_name.to_string())
                .or_insert(Branch::Missing(up_name.to_string()));

            downstream_lookup
                .entry(up_name.to_string())
                .or_default()
                .push(branch.name().to_string())
        }
    }

    let mut nodes_by_name: IndexMap<String, Rc<RefCell<Tree<Branch>>>> = Default::default();

    for branch in branches.values() {
        if !downstream_lookup.contains_key(&branch.name().to_string()) {
            nodes_by_name.insert(
                branch.name().to_string(),
                Rc::new(RefCell::new(Tree::Leaf(branch.clone()))),
            );
        }
    }

    let mut to_process: VecDeque<String> = branches.keys().cloned().collect();

    while let Some(next_branch) = to_process.pop_front() {
        if nodes_by_name.contains_key(&next_branch) {
            continue;
        }

        let downstreams = downstream_lookup.get(&next_branch).unwrap();
        if downstreams.iter().all(|it| nodes_by_name.contains_key(it)) {
            let mut children: Vec<Rc<RefCell<Tree<Branch>>>> = vec![];
            for dwn in downstreams {
                children.push(nodes_by_name.get(dwn).unwrap().clone())
            }
            let node = Tree::Node {
                value: branches.get(&next_branch).cloned().unwrap(),
                children,
            };
            nodes_by_name.insert(next_branch, Rc::new(RefCell::new(node)));
        } else {
            to_process.push_back(next_branch);
        }
    }

    let roots = branches
        .values()
        .filter(|it| !it.has_upstream())
        .map(|it| nodes_by_name.get(it.name()).unwrap().clone())
        .collect();
    Ok(roots)
}

const ORIGIN_COLOR: comfy_table::Color = comfy_table::Color::DarkBlue;

fn print_branch_tree() -> GEResult<()> {
    let roots = build_branch_trees()?;

    let mut all_rows = vec![];

    for root in roots {
        let formatted = draw_from(&*root.borrow(), 2, String::default(), String::default());
        for (branch, entry) in formatted {
            let curr = match branch {
                Branch::Branch { name, .. } if name.contains("origin") => {
                    vec![
                        Cell::new(entry).fg(ORIGIN_COLOR),
                        Cell::new(""),
                        Cell::new(""),
                        Cell::new(""),
                        Cell::new(""),
                    ]
                }
                Branch::Branch {
                    sha,
                    status,
                    message,
                    current,
                    ..
                } => {
                    vec![
                        Cell::new(entry),
                        Cell::new(sha),
                        Cell::new(
                            status
                                .and_then(|it| it.ahead)
                                .map(|it| format!("+{it}"))
                                .unwrap_or("".to_string()),
                        )
                        .fg(comfy_table::Color::DarkGreen),
                        Cell::new(
                            status
                                .and_then(|it| it.behind)
                                .map(|it| format!("-{it}"))
                                .unwrap_or("".to_string()),
                        )
                        .fg(comfy_table::Color::Red),
                        if current {
                            Cell::new(message).fg(comfy_table::Color::DarkGreen)
                        } else {
                            Cell::new(message)
                        },
                    ]
                }
                Branch::Missing(name) => {
                    vec![
                        Cell::new(entry).fg(if name.contains("origin") {
                            ORIGIN_COLOR
                        } else {
                            comfy_table::Color::Red
                        }),
                        Cell::new(""),
                        Cell::new(""),
                        Cell::new(""),
                        Cell::new(""),
                    ]
                }
            };
            all_rows.push(curr);
        }
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
    run_git(&["branch", "-D", branch], verbose)?;
    Ok(())
}

fn purge(prefix: &str, no_confirm: bool, verbose: bool) -> GEResult<()> {
    let re = Regex::new(&format!(r"origin/{}/([\w-]+)", prefix))?;
    let branches: std::vec::Vec<String> = run_git(&["remote", "prune", "origin", "-n"], verbose)?
        .lines()
        .map(|s| s.trim())
        .filter_map(|s| re.captures(s))
        .filter_map(|cap| cap.get(1))
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
        run_git(&["remote", "prune", "origin"], verbose)?;
    } else if Confirm::new().with_prompt("Ok?").interact()? {
        for branch in branches {
            let result = delete_branch(&branch, true);
            if let Err(e) = result {
                println!("Warning: ignoring error deleting branch {}: {}", branch, e)
            }
        }
        run_git(&["remote", "prune", "origin"], verbose)?;
    } else {
        println!("Cancelling.")
    }

    Ok(())
}

fn add_amend_push_origin(verbose: bool) -> GEResult<()> {
    run_git(&["add", "."], true)?;
    run_git(&["commit", "--amend", "--no-edit"], true)?;
    push_origin(verbose)
}

fn rebase_onto_latest(branch: &str, verbose: bool) -> GEResult<()> {
    let curr = get_curr_branch(false)?;
    run_git(&["checkout", branch], true)?;
    run_git(&["pull", "--ff-only"], true)?;
    run_git(&["checkout", &curr], true)?;
    fix_upstream(branch, verbose)
}

fn reset_hard_origin(verbose: bool) -> GEResult<()> {
    let curr = get_curr_branch(verbose)?;
    ensure_clean()?;
    run_git(&["fetch", "origin"], true)?;
    run_git(&["reset", "--hard", &format!("origin/{curr}")], true)?;
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
