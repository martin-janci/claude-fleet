# PhoneRow

`SessionRow` at phone size: dot, title and age, then what the session waits on, then at most two chips or row actions. Also used for tasks, hosts and files.

**The consumer provides** the status, title, age, line two and optional chips or `Button` `sm` actions (36 px tall inside a 48 px row zone).

**Do**
- Lead line two with the status word: "Waiting for you: approve push to main", "Failed: tests crashed", "Signal lost · last seen 11:52".
- Group by host, project, urgency or work with section headers, so the host does not eat line two.
- Show stale data with its age and "Was working", with no spinner.

**Don't**
- Don't put Approve on a row or a swipe; answering a permission opens the session's question card.
- Don't use red for attention; red is Failed only.
