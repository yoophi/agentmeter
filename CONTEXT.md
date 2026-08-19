# Agent Usage Meter

Agentmeter presents locally available usage limits from coding-agent providers and preserves their history across runs.

## Language

**Usage Limit**:
A provider-defined allowance whose consumed share is reported as a percentage.
_Avoid_: Meter, quota bar

**Usage Window**:
The fixed interval over which a Usage Limit accumulates before its provider-defined reset time.
_Avoid_: Session, period

**Window History**:
The measured Usage Limit samples belonging to one Usage Window, identified by its duration and reset time.
_Avoid_: Chart cache, series file

**Partial Restore**:
A restoration result that retains every valid Window History while reporting invalid histories as warnings.
_Avoid_: Successful restore, silent skip

**Refresh Request**:
A request to acquire the latest available usage snapshot; Fresh requests take precedence when pending requests are combined.
_Avoid_: Reload, poll event
