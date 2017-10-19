package main

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"os/exec"
	"regexp"
	"strings"
	"text/tabwriter"

	docopt "github.com/docopt/docopt-go"
	"github.com/mgutz/ansi"
)

func rungit(cmdargs []string, verbose bool) string {
	cmd := "git"
	if verbose {
		fmt.Println(ansi.Color("cmd", "white+b:green") + " " +
			cmd + " " + strings.Join(cmdargs, " "))
	}
	cmdObj := exec.Command(cmd, cmdargs...)
	cmdOutput, err := cmdObj.Output()
	if exiterr, ok := err.(*exec.ExitError); ok {
		fmt.Println(string(exiterr.Stderr))
		os.Exit(1)
	} else if err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
	if verbose {
		fmt.Println(string(cmdOutput))
	}
	return strings.TrimSpace(string(cmdOutput))
}

func lasthash(verbose bool) string {
	return rungit([]string{"log", "-n", "1", "--pretty=format:%H"}, verbose)
}

func ensureClean() {
	status := rungit([]string{"status"}, false)
	if !(strings.Contains(status, "nothing to commit, working directory clean") ||
		strings.Contains(status, "nothing to commit, working tree clean")) {
		fmt.Println(ansi.Color(status, "white:red"))
		os.Exit(1)
	}
}

func handleSubmodules(verbose bool) {
	rungit([]string{"submodule", "init"}, verbose)
	rungit([]string{"submodule", "update", "--recursive"}, verbose)
}

func getUpstream(verbose bool) string {
	return rungit([]string{"rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"}, verbose)
}

func getCurrBranch(verbose bool) string {
	return rungit([]string{"rev-parse", "--abbrev-ref", "HEAD"}, verbose)
}

func fixUpstream(upstream string, verbose bool) {
	commit := lasthash(verbose)
	rungit([]string{"branch", "--set-upstream-to", upstream}, true)
	ensureClean()
	rungit([]string{"reset", "--hard", upstream, "--"}, true)
	handleSubmodules(true)
	rungit([]string{"cherry-pick", commit}, true)
	handleSubmodules(true)
}

func checkout(branch string, verbose bool) {
	rungit([]string{"checkout", branch}, verbose)
	handleSubmodules(verbose)
}

func recFixUp(terminal string, verbose bool, branchCache []string) {
	currBranch := getCurrBranch(verbose)
	if currBranch == terminal {
		for _, branch := range branchCache {
			checkout(branch, true)
			fixUpstream(getUpstream(false), verbose)
		}
		return
	}
	currUpstream := getUpstream(verbose)
	checkout(currUpstream, false)
	recFixUp(terminal, verbose, append([]string{currBranch}, branchCache...))
}

func commitBranch(branchName string, verbose bool) {
	rungit([]string{"branch", branchName}, true)
	ensureClean()
	rungit([]string{"reset", "--hard", "HEAD~1"}, true)
	rungit([]string{"checkout", branchName}, true)
	handleSubmodules(true)
}

type branchT struct {
	Desc        branchDescriptor
	Downstream  []*branchT
	HasUpstream bool
}

type branchDescriptor struct {
	Current  bool
	Name     string
	Sha      string
	Upstream string
	Status   string
	Message  string
}

func parseBranchEntry(branchEntry string) branchDescriptor {
	descriptor := branchDescriptor{}
	descriptor.Current = string(branchEntry[0]) == "*"
	whitespace := regexp.MustCompile("\\s+")
	parts := whitespace.Split(strings.TrimLeft(branchEntry, "* "), 3)
	descriptor.Name = parts[0]
	descriptor.Sha = parts[1]
	rest := parts[2]

	restExpr := regexp.MustCompile("\\[([^\\]]*)\\] (.*)")
	m := restExpr.FindStringSubmatch(rest)
	if m == nil {
		panic(fmt.Sprintf("Unexpectedly unable to parse branch line %s\n", branchEntry))
	} else {
		descriptor.Message = m[2]
		upstreamAndMaybeStatus := strings.Split(m[1], ": ")
		descriptor.Upstream = upstreamAndMaybeStatus[0]
		if len(upstreamAndMaybeStatus) > 1 {
			descriptor.Status = upstreamAndMaybeStatus[1]
		}
	}
	return descriptor
}

var indentAmount = 4

func prefixForDepth(depth int) string {
	return strings.Repeat(" ", indentAmount*depth) + "+-- "
}

func printTreeRootedAt(w io.Writer, root *branchT, currDepth int) {
	if currDepth == 0 {
		outputLine := prefixForDepth(currDepth) + root.Desc.Upstream
		if strings.HasPrefix(root.Desc.Upstream, "origin") {
			fmt.Fprintln(w, ansi.Color(outputLine+"\t\t\t", "blue"))
		} else {
			fmt.Fprintln(w, ansi.Color(outputLine+" [missing]\t\t\t", "red"))
		}
		printTreeRootedAt(w, root, currDepth+1)
		return
	}
	prefix := prefixForDepth(currDepth) + root.Desc.Name
	outputLine := prefix + "\t" + root.Desc.Sha + "\t" + root.Desc.Message + "\t"
	fmt.Fprintln(w, outputLine)
	for _, ds := range root.Downstream {
		printTreeRootedAt(w, ds, currDepth+1)
	}
}

func drawBranchTree() {
	branches := strings.Split(rungit([]string{"branch", "-vv"}, false), "\n")
	branchMap := map[string]*branchT{}
	for _, br := range branches {
		desc := parseBranchEntry(br)
		branchMap[desc.Name] = &branchT{Desc: desc, Downstream: []*branchT{}, HasUpstream: false}
	}
	for _, br := range branchMap {
		if upstreamBranch, exists := branchMap[br.Desc.Upstream]; exists {
			upstreamBranch.Downstream = append(branchMap[br.Desc.Upstream].Downstream, br)
			branchMap[br.Desc.Upstream] = upstreamBranch
			br.HasUpstream = true
		}
	}
	w := new(tabwriter.Writer)
	outputBuffer := bytes.Buffer{}
	w.Init(&outputBuffer, 5, 0, 1, ' ', 0)
	for _, br := range branchMap {
		if !br.HasUpstream {
			printTreeRootedAt(w, br, 0)
		}
	}
	w.Flush()
	output := outputBuffer.String()
	// Finally, we need to highlight the current branch in green.
	// We couldn't do this earlier since the nonprinting escape characters
	// count as characters for balancing columns.
	branchExtractRe := regexp.MustCompile("\\+-- ([^\\s]+)")
	for _, line := range strings.Split(output, "\n") {
		match := branchExtractRe.FindStringSubmatch(line)
		if match == nil {
			continue
		}
		lineBranch := match[1]
		if brT, exists := branchMap[lineBranch]; exists && brT.Desc.Current {
			fmt.Println(ansi.Color(line, "green"))
		} else {
			fmt.Println(line)
		}
	}
}

func main() {
	usage := `git_ext - a grab bag of git shortcuts

Usage:
	git_ext [--verbose] (lh | lasthash)
	git_ext [--verbose] shup | show_up
	git_ext [--verbose] fu | fix_up | fix_upstream
	git_ext [--verbose] up <branch>
	git_ext [--verbose] (rup | rec_fix_up) <terminal_branch>
	git_ext [--verbose] (cbr | commit_br) <branch>
	git_ext [--verbose] tree | show_tree

Options:
	--verbose  		Show extra output?

Commands:
	lh, lasthash                Print the most recent commit's hash
	shup, show_up               Print the upstream branch
	fu, fix_up, fix_upstream    reset to just the lastest commit on top of the upstream branch
	up                          set upstream, then run fix_up
	rup, rec_fix_up             recursively apply fix_upstream from terminal_branch to this one
	cbr, commit_br              create a new branch at the current commit, reset to HEAD~1, check out the new branch
	tree, show_tree             draw the current tree of branches
	`

	args, err := docopt.Parse(usage, nil, true, "0.0.1", true)
	if err != nil {
		panic(err)
	}

	flag := func(names ...string) bool {
		for _, name := range names {
			if args[name] == true {
				return true
			}
		}
		return false
	}

	verbose := flag("verbose")

	if flag("lh", "lasthash") {
		fmt.Println(lasthash(verbose))
		return
	}

	if flag("shup", "show_upstream") {
		return
	}

	if flag("fu", "fix_up", "fix_upstream") {
		fixUpstream(getUpstream(verbose), verbose)
		return
	}

	if flag("up") {
		fixUpstream(args["<branch>"].(string), verbose)
		return
	}

	if flag("rup", "rec_fix_up") {
		recFixUp(args["<terminal_branch>"].(string), verbose, []string{})
		return
	}

	if flag("cbr", "commit_br") {
		commitBranch(args["<branch>"].(string), verbose)
	}

	if flag("tree", "show_tree") {
		drawBranchTree()
		return
	}
}
