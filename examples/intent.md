# Add rate limiting to the login endpoint

Context for reviewers: brute-force attempts were seen in the auth logs.

## Acceptance criteria

- [ ] More than 5 failed logins per minute from one IP are rejected with HTTP 429
- [ ] The limit and window are read from configuration, not hardcoded
- [ ] Successful logins do not count against the limit
- Rejected attempts are logged without the submitted password

### Scenario: lockout expires

Given an IP that has been rate limited
When the window passes
Then the next login attempt is evaluated normally
