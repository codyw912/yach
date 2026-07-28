Scenario driven by run.sh (two `yach run --session` invocations —
gate-runner-only until the harbor path supports multi-shot tasks):

Turn 1: Create a file named journal.txt whose content is exactly the
single line: alpha

Turn 2: Add a second line to the file you created in the previous
turn: beta

Turn 2 never names the file — it is only satisfiable from session
context, which is what this task gates (#192).
