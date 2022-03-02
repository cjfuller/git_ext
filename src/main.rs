use std::collections::HashMap;
use std::iter::Iterator;
use std::process::Command;

use anyhow::Error;
use colored::*;
use dialoguer::Confirm;
use regex::Regex;
use structopt::StructOpt;
use tabular::{Row, Table};

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
        return Err(Error::msg(status.white().on_red()));
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

fn rec_fix_up(terminal: &str, verbose: bool, branch_cache: &mut Vec<String>) -> GEResult<()> {
    let curr_branch = get_curr_branch(verbose)?;
    if curr_branch == terminal {
        for branch in branch_cache {
            checkout(branch, true)?;
            fix_upstream(&get_upstream(false)?, verbose)?;
        }
        return Ok(());
    }
    let curr_upstream = get_upstream(verbose)?;
    checkout(&curr_upstream, false)?;
    branch_cache.insert(0, curr_branch);
    rec_fix_up(terminal, verbose, branch_cache)
}

fn commit_branch(branch_name: &str, _verbose: bool) -> GEResult<()> {
    run_git(vec!["branch", branch_name], true)?;
    ensure_clean()?;
    run_git(vec!["reset", "--hard", "HEAD~1"], true)?;
    run_git(vec!["checkout", branch_name], true)?;
    handle_submodules(true)
}

fn push_origin(verbose: bool) -> GEResult<()> {
    let branch = get_curr_branch(verbose)?;
    run_git(vec!["push", "-f", "origin", &branch], true)?;
    Ok(())
}

#[derive(Clone, Debug)]
struct BranchDescriptor {
    current: bool,
    name: String,
    sha: String,
    upstream: Option<String>,
    message: String,
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
    let status = upstream_and_maybe_status.and_then(|v| v.get(1).copied());

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
        status: String::from(status.unwrap_or("")),
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
) -> GEResult<Vec<Row>> {
    let depth = branch_depth(branches_by_name, &root.desc.name);
    let prefix = prefix_for_depth(depth) + if root.desc.current { "* " } else { "" };
    let upstream_prefix = prefix_for_depth(depth - 1);

    let mut output_rows = if let Some(up) = &root.desc.upstream {
        if up.contains("origin") {
            vec![Row::new()
                .with_cell((upstream_prefix + up).blue())
                .with_cell("")
                .with_cell("")]
        } else if !branches_by_name.contains_key(up) {
            vec![Row::new()
                .with_cell((upstream_prefix + up + " [missing]").red())
                .with_cell("")
                .with_cell("")]
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    output_rows.append(&mut vec![Row::new()
        .with_cell(prefix + &root.desc.name)
        .with_cell(root.desc.sha.clone())
        .with_cell(if root.desc.current {
            root.desc
                .message
                .chars()
                .take(40)
                .collect::<String>()
                .green()
        } else {
            Colorize::clear(&*root.desc.message.chars().take(40).collect::<String>())
        })]);
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

    let mut all_rows: Vec<Row> = vec![];

    for br in root_branches {
        all_rows.append(&mut format_tree_rooted_at(&branches_by_name, &br)?)
    }

    let mut table = Table::new("{:<}  {:<} {:<}");
    for row in all_rows {
        table.add_row(row);
    }
    println!("{}", table);

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

#[derive(Debug, StructOpt)]
pub enum SubCommand {
    #[structopt(alias = "lh")]
    Lasthash {},

    #[structopt(alias = "shup")]
    ShowUp {},

    #[structopt(alias = "fu")]
    FixUp {},

    Up {
        branch: String,
    },

    #[structopt(alias = "rup")]
    RecFixUp {
        terminal: String,
    },

    #[structopt(alias = "cbr")]
    CommitBr {
        name: String,
    },

    #[structopt(alias = "tree")]
    ShowTree {},

    #[structopt(alias = "po")]
    PushOrigin {},

    Purge {
        prefix: String,
        #[structopt(short = "y")]
        no_confirm: bool,
    },

    #[structopt(alias = "aap")]
    AddAmendPushOrigin {},

    #[structopt(alias = "rl")]
    RebaseOntoLatest {
        branch: Option<String>,
    },
}

#[derive(Debug, StructOpt)]
pub struct GitExt {
    #[structopt(short, long)]
    verbose: bool,
    #[structopt(subcommand)]
    cmd: SubCommand,
}

fn main() {
    let opt = GitExt::from_args();
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
        RecFixUp { terminal } => rec_fix_up(&terminal, verbose, &mut vec![]),
        CommitBr { name } => commit_branch(&name, verbose),
        PushOrigin {} => push_origin(verbose),
        ShowTree {} => print_branch_tree(),
        Purge { prefix, no_confirm } => purge(&prefix, no_confirm, verbose),
        AddAmendPushOrigin {} => add_amend_push_origin(verbose),
        RebaseOntoLatest { branch } => {
            rebase_onto_latest(&branch.unwrap_or("master".to_string()), verbose)
        }
    };
    if result.is_err() {
        eprintln!("{}", result.unwrap_err());
        std::process::exit(1)
    }
}
