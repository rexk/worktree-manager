As things get more and more complicated, and we are using more LLM agents to handle multiple works simultaneously, I'm running into issues with working with the single Git repository folder. 
I tried to manage this by cloning into different directories and then working in different directories simultaneously, but this became very difficult to manage multiple workspaces at the same time.
I have to manage and remember which folder is containing what work over different teamwork sessions, and it's quite a time-consuming and difficult to context-switch because I need to remember many things at the same time.

I want to explore some options for streamlining this workflow.
For this planning, I want to be scoped into exploring altering Git worktree, To come up with a similar function so that I can have a fresh copy of a working branch isolated, and can have a class session for each of them. 
However, there are some known limitations around Git worktrees that I want to be able to address as we go along. Or even if there are different ways we should be doing this, such as manually copying and pasting these different changes, we should do that.

 There are two major limitations that I'm seeing from Git Worktree:
1. We can only have one branch checked out on a single directory.- Actually, let me rephrase that. What I meant to say is that if I have a working directory with the branch "A" and I'm trying to create another directory with the same branch "A", I can't do that because Git worktree can only have a unique branch per worktree. 
2. Above is creating interesting limitations as I if I want to swap my current working directory into the branch that is already checked out into the different work tree, I can do that because there's already a working tree with that branch. 
3. And this creates a unique constraint because in my usual workflow, I want to maintain a single interactive working directory that I am tying that into my editors or main class session to work with, But as needed, if I want to branch off to create a pair of work that I would like to do that in a different directory, right? And then, as I work through on these works, I want to be able to swap that working branch into my main work tree so that I can validate things manually for the tools that I have rather than spotting up multiple IDs for the different directors and stuff. 

So I want to be able to come up with some practical solutions, or even a midterm solution that we can think of managing this ourselves.
To be more precise, I think the following are the functional requirements that I'm looking for:

1. There will be one major entrypoint that we are using for most of interactive work, if needed - hooking up with IDE as such as:
   e.g) ~/workspace/<repo-name>
2. There are two ways of branching off - usual git checkout operation, and one that creates worktree or clone of branch into different directory
3. There should be reasonable folder structure conventions in known places so that we can track this. 
4. We probably also need to track the state of this different worktree and how these branches are interacting with each other as a graph or something, so that the syncing between base branch and child branches are relatively easy. 
5. Regardless of a branch being in the worktree vs local branch, we should be able to swap the branch easily


I believe you can get a lot of inspiration from Graphite CLI, and Git worktree, for managing these multiple branches. 
I also thought about using bare repos to manage that, but let me know if that's feasible.

I imagine the end product would be some sort of a CLI or even some shell scripts that's going to help us transition this workflow, easily


