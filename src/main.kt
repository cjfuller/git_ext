package io.cjf.git_ext

import com.github.ajalt.clikt.core.CliktCommand
import com.github.ajalt.clikt.core.requireObject
import com.github.ajalt.clikt.core.subcommands
import com.github.ajalt.clikt.parameters.arguments.argument
import com.github.ajalt.clikt.parameters.options.flag
import com.github.ajalt.clikt.parameters.options.option
import com.github.ajalt.mordant.components.Padding
import com.github.ajalt.mordant.rendering.OverflowWrap
import com.github.ajalt.mordant.rendering.TextAlign
import com.github.ajalt.mordant.table.Borders
import com.github.ajalt.mordant.table.ColumnWidth
import com.github.ajalt.mordant.table.table
import com.github.ajalt.mordant.terminal.Terminal
import com.github.ajalt.mordant.terminal.TextColors.blue
import com.github.ajalt.mordant.terminal.TextColors.brightWhite
import com.github.ajalt.mordant.terminal.TextColors.green
import com.github.ajalt.mordant.terminal.TextColors.red
import com.github.ajalt.mordant.terminal.TextColors.white

val t = Terminal()

sealed class Result<T> {
    abstract fun <U> map(f: (T) -> U): Result<U>
    abstract fun unwrap(): T
    operator fun not(): T = unwrap()
    abstract fun isFailure(): Boolean
    data class Success<T>(val value: T) : Result<T>() {
        override fun <U> map(f: (T) -> U): Result<U> = Success(f(value))
        override fun unwrap() = value
        override fun isFailure() = false
    }

    data class Failure<T>(val exception: Throwable) : Result<T>() {
        override fun <U> map(f: (T) -> U): Result<U> = Failure(exception)
        override fun unwrap() = throw exception
        override fun isFailure() = true
    }

    companion object {
        fun <U> of(f: () -> U): Result<U> = try {
            Success(f())
        } catch (e: Exception) {
            Failure(e)
        }
    }
}

fun Any?.discard() = Unit

class GitError(exitCode: Int, stderr: String) :
    Exception("Git exited with status $exitCode:\n$stderr")

fun runGit(cmdargs: List<String>, verbose: Boolean = false): Result<String> {
    val cmdString = "${(brightWhite on green)("git")} ${cmdargs.joinToString(" ")}"
    if (verbose) {
        t.println(cmdString)
    }

    val proc = Runtime.getRuntime().exec((listOf("git") + cmdargs).toTypedArray())
    val exitStatus = proc.waitFor()
    if (exitStatus != 0) {
        return Result.Failure(
            GitError(
                exitStatus,
                proc.errorStream.reader().readText()
            )
        )
    }
    val output = proc.inputStream.reader().readText().trim()
    if (verbose) {
        t.println(output)
    }
    return Result.Success(output)
}

fun lasthash(verbose: Boolean): Result<String> =
    runGit(listOf("log", "-n", "1", "--pretty=format:%H"), verbose)

fun ensureClean(): Result<Unit> = Result.of {
    val status = runGit(listOf("status"), false).unwrap()
    if ("nothing to commit, working directory clean" !in status &&
        "nothing to commit, working tree clean" !in status
    ) {
        throw Exception(
            "Aborting due to unclean repository:\n${
                (white on red)(
                    status
                )
            }"
        )
    }
}

fun handleSubmodules(verbose: Boolean): Result<Unit> = Result.of {
    !runGit(listOf("submodule", "init"), verbose)
    !runGit(listOf("submodule", "update", "--recursive"), verbose)
    Unit
}

fun getUpstream(verbose: Boolean): Result<String> =
    runGit(listOf("rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"), verbose)

fun getCurrBranch(@Suppress("UNUSED_PARAMETER") verbose: Boolean): Result<String> =
    runGit(listOf("rev-parse", "--abbrev-ref", "HEAD"))

fun fixUpstream(upstream: String, verbose: Boolean): Result<Unit> = Result.of {
    val commit = !lasthash(verbose)
    !runGit(listOf("branch", "--set-upstream-to", upstream), true)
    !ensureClean()
    !runGit(listOf("reset", "--hard", upstream, "--"), true)
    !handleSubmodules(true)
    !runGit(listOf("cherry-pick", commit), true)
    !handleSubmodules(true)
}

fun commitBranch(
    branch: String,
    @Suppress("UNUSED_PARAMETER") verbose: Boolean
): Result<Unit> = Result.of {
    !runGit(listOf("branch", "--track", branch), true)
    !ensureClean()
    !runGit(listOf("reset", "--hard", "HEAD~1"), true)
    !runGit(listOf("checkout", branch), true)
    !handleSubmodules(true)
}

fun pushOrigin(verbose: Boolean): Result<Unit> = Result.of {
    val branch = !getCurrBranch(verbose)
    !runGit(listOf("push", "-f", "origin", branch), true)
}

data class BranchDescriptor(
    val current: Boolean,
    val name: String,
    val sha: String,
    val upstream: String?,
    val status: String,
    val message: String,
)

data class BranchT(
    val desc: BranchDescriptor,
    val downstream: List<String>
) {
    fun hasUpstream(): Boolean = desc.upstream != null
}

fun branchDepth(branchesByName: Map<String, BranchT>, branchName: String): Int {
    val up = branchesByName[branchName]?.desc?.upstream
    return if (up != null) {
        1 + branchDepth(branchesByName, up)
    } else {
        0
    }
}

class ParseError(msg: String) : Exception("Branch parsing error: $msg")

fun parseBranchEntry(branchEntry: String): Result<BranchDescriptor> = Result.of {
    val whitespace = Regex("""\s+""")
    val parts = branchEntry.trim().trimStart('*').trim().split(whitespace, 3)
    if (parts.size != 3) {
        throw ParseError("Wrong number of parts in $branchEntry")
    }
    val (name, sha, rest) = parts
    val restExpr = Regex("""(?:\[([^]]*)] )?(.*)""")
    val groups = restExpr.find(rest)?.groupValues
        ?: throw ParseError("Failed to parse $rest")
    val upstreamAndMaybeStatus = groups.get(1).split(": ")
    val upstream = upstreamAndMaybeStatus[0]
    val status = upstreamAndMaybeStatus.getOrNull(1) ?: ""
    BranchDescriptor(
        current = branchEntry[0] == '*',
        name = name,
        sha = sha,
        message = groups.getOrElse(2) { "no message" },
        upstream = upstream,
        status = status
    )
}

const val INDENT_AMOUNT = 2

fun prefixForDepth(depth: Int): String = if (depth <= 0) {
    ""
} else {
    " ".repeat(
        INDENT_AMOUNT * depth
    ) + "+-- "
}

fun formatTreeRootedAt(
    branchesByName: Map<String, BranchT>,
    root: BranchT
): Result<List<List<String>>> = Result.of {
    val depth = branchDepth(branchesByName, root.desc.name)
    val prefix = prefixForDepth(depth) + if (root.desc.current) {
        "* "
    } else {
        ""
    }
    val upstreamPrefix = prefixForDepth(depth - 1)

    when {
        "origin" in root.desc.upstream ?: "" -> listOf(
            listOf(
                blue(upstreamPrefix + root.desc.upstream),
                "",
                ""
            )
        )
        root.desc.upstream !in branchesByName -> listOf(
            listOf(
                red(upstreamPrefix + root.desc.upstream + " [missing]"),
                "",
                ""
            )
        )
        else -> listOf()
    } + listOf(
        listOf(
            prefix + root.desc.name,
            root.desc.sha,
            if (root.desc.current) {
                green(root.desc.message)
            } else {
                root.desc.message
            }
        )
    ) + root.downstream.flatMap { down ->
        branchesByName[down]
            ?.let { !formatTreeRootedAt(branchesByName, it) }
            ?: listOf()
    }
}

fun printBranchTree(): Result<Unit> = Result.of {
    val branchGit = runGit(listOf("branch", "-vv"), false)
        .unwrap()
        .lines()

    val branchesWithoutDownstream = branchGit.map(::parseBranchEntry).map {
        BranchT(!it, downstream = listOf())
    }

    val branchDownstreamMap = mutableMapOf<String, List<String>>()
    branchesWithoutDownstream.forEach {
        val upstream = it.desc.upstream
        if (upstream != null) {
            branchDownstreamMap[upstream] =
                (branchDownstreamMap[upstream] ?: listOf()) + it.desc.name
        }
    }

    val branches = branchesWithoutDownstream.map {
        it.copy(downstream = branchDownstreamMap[it.desc.name] ?: listOf())
    }

    val branchesByName = branches.map { it.desc.name to it }.toMap()
    val roots = branches
        .filter { !it.hasUpstream() || it.desc.upstream !in branchesByName }
        .sortedBy { it.desc.name }

    t.println(table {
        borders = Borders.NONE
        column(0) {
            align = TextAlign.LEFT
            width = ColumnWidth.Expand()
        }
        column(1) {
            align = TextAlign.RIGHT
            width = ColumnWidth.Fixed(8)
            overflowWrap = OverflowWrap.TRUNCATE
            padding = Padding.none()
        }
        column(2) {
            align = TextAlign.LEFT
            width = ColumnWidth.Expand()
        }
        body {
            roots.flatMap { !formatTreeRootedAt(branchesByName, it) }
                .map { r -> row(*r.toTypedArray()) }
        }
    })
}

fun deleteBranch(branch: String, verbose: Boolean): Result<Unit> =
    runGit(listOf("branch", "-D", branch), verbose).map(Any::discard)

fun purge(prefix: String, verbose: Boolean): Result<Unit> =
    Result.of {
        val re = Regex("""origin/$prefix/([\w-]+)""")
        val branches = runGit(listOf("remote", "prune", "origin", "-n"), verbose)
            .unwrap()
            .lines()
            .map(String::trim)
            .mapNotNull { re.find(it)?.groupValues }
            .mapNotNull { it.getOrNull(1) }
            .map { "$prefix/$it" }

        val results = branches.map { deleteBranch(it, true) }
        val allMessages = results.fold(listOf<String>()) { acc, res ->
            if (res is Result.Failure) {
                acc + (res.exception.message ?: "(unknown error)")
            } else {
                acc
            }
        }
        if (allMessages.isNotEmpty()) {
            val errorMessage =
                "Got the following errors deleting branches:\n${
                    allMessages.joinToString("\n")
                }"
            throw Exception(errorMessage)
        }
    }

data class VerboseFlag(val value: Boolean = false)

class Lasthash : CliktCommand() {
    val verbose by requireObject<VerboseFlag>()
    override fun run() = t.println(!lasthash(verbose.value))
}

class ShowUp : CliktCommand() {
    val verbose by requireObject<VerboseFlag>()
    override fun run() = t.println(!getUpstream(verbose.value))
}

class FixUp : CliktCommand() {
    val verbose by requireObject<VerboseFlag>()
    override fun run() = !fixUpstream(!getUpstream(verbose.value), verbose.value)
}

class Up : CliktCommand() {
    val verbose by requireObject<VerboseFlag>()
    val branch: String by argument()
    override fun run() = !fixUpstream(branch, verbose.value)
}

class CommitBranch : CliktCommand() {
    val verbose by requireObject<VerboseFlag>()
    val name: String by argument()
    override fun run() = !commitBranch(name, verbose.value)
}

class ShowTree : CliktCommand() {
    val verbose by requireObject<VerboseFlag>()
    override fun run() = !printBranchTree()
}

class PushOrigin : CliktCommand() {
    val verbose by requireObject<VerboseFlag>()
    override fun run() = !pushOrigin(verbose.value)
}

class Purge : CliktCommand() {
    val verbose by requireObject<VerboseFlag>()
    val prefix: String by argument()
    override fun run() = !purge(prefix, verbose.value)
}

class GitExt : CliktCommand() {
    val verbose: Boolean by option().flag("verbose")
    override fun aliases() = mapOf(
        "shup" to listOf("show-up"),
        "lh" to listOf("lasthash"),
        "fu" to listOf("fix-up"),
        "cbr" to listOf("commit-branch"),
        "tree" to listOf("show-tree"),
        "po" to listOf("push-origin"),
    )

    override fun run() {
        currentContext.findOrSetObject { VerboseFlag(verbose) }
    }
}

fun main(args: Array<String>) = GitExt().subcommands(
    Lasthash(),
    ShowUp(),
    FixUp(),
    Up(),
    CommitBranch(),
    PushOrigin(),
    ShowTree(),
    Purge()
).main(args)
