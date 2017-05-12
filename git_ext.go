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

	"github.com/mgutz/ansi"
	cli "gopkg.in/urfave/cli.v1"
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
	var verbose = false
	cli.VersionFlag = cli.BoolFlag{
		Name:  "print-version, V",
		Usage: "print only the version",
	}
	app := cli.NewApp()

	app.Name = "git_ext"
	app.Usage = "a grab bag of git shortcuts"

	app.Flags = []cli.Flag{
		cli.BoolFlag{
			Name:        "verbose, v",
			Usage:       "Show extra output?",
			Destination: &verbose,
		},
	}

	app.Commands = []cli.Command{
		{
			Name:    "lasthash",
			Aliases: []string{"lh"},
			Usage:   "print the most recent commit's hash",
			Action: func(_ *cli.Context) error {
				fmt.Println(lasthash(verbose))
				return nil
			},
		},
		{
			Name:    "show_up",
			Aliases: []string{"shup"},
			Usage:   "print the upstream branch",
			Action: func(_ *cli.Context) error {
				fmt.Println(getUpstream(verbose))
				return nil
			},
		},
		{
			Name:    "fix_upstream",
			Aliases: []string{"fix_up", "fu"},
			Usage:   "reset to just latest commit on top of the current branch",
			Action: func(c *cli.Context) error {
				upstream := getUpstream(verbose)
				fixUpstream(upstream, verbose)
				return nil
			},
		},
		{
			Name:      "up",
			Usage:     "set the upstream to the specified branch, reset to latest commit on top of that upstream",
			ArgsUsage: "[branch]",
			Action: func(c *cli.Context) error {
				upstream := c.Args().Get(0)
				if upstream == "" {
					return cli.NewExitError("upstream must be specified", 1)
				}
				fixUpstream(upstream, verbose)
				return nil
			},
		},
		{
			Name:      "rec_fix_up",
			Aliases:   []string{"rup"},
			Usage:     "recursively apply fix_upstream until we're on this branch",
			ArgsUsage: "[terminal_branch]",
			Action: func(c *cli.Context) error {
				terminal := c.Args().Get(0)
				if terminal == "" {
					return cli.NewExitError("terminal branch must be specified", 1)
				}
				recFixUp(terminal, verbose, []string{})
				return nil
			},
		},
		{
			Name:    "show_tree",
			Aliases: []string{"tree"},
			Usage:   "draw the current tree of branches",
			Action: func(c *cli.Context) error {
				drawBranchTree()
				return nil
			},
		},
	}

	app.Run(os.Args)
}
