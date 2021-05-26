# Session Module

The Session module allows validators to manage their session keys, provides a function for changing
the session length, and handles session rotation.

- [`session::Trait`](https://docs.rs/pallet-session/latest/pallet_session/trait.Trait.html)
- [`Call`](https://docs.rs/pallet-session/latest/pallet_session/enum.Call.html)
- [`Module`](https://docs.rs/pallet-session/latest/pallet_session/struct.Module.html)

## Overview

### Terminology
<!-- Original author of paragraph: @gavofyork -->

- **Session:** A session is a period of time that has a constant set of validators. Validators can only join
or exit the validator set at a session change. It is measured in block numbers. The block where a session is
ended is determined by the `ShouldEndSession` trait. When the session is ending, a new validator set
can be chosen by `OnSessionEnding` implementations.
- **Session key:** A session key is actually several keys kept together that provide the various signing
functions required by network authorities/validators in pursuit of their duties.
- **Validator ID:** Every account has an associated validator ID. For some simple staking systems, this
may just be the same as the account ID. For staking systems using a stash/controller model,
the validator ID would be the stash account ID of the controller.
- **Session key configuration process:** Session keys are set using `set_keys` for use not in
the next session, but the session after next. They are stored in `NextKeys`, a mapping between
the caller's `ValidatorId` and the session keys provided. `set_keys` allows users to set their
session key prior to being selected as validator.
It is a public call since it uses `ensure_signed`, which checks that the origin is a signed account.
As such, the account ID of the origin stored in `NextKeys` may not necessarily be associated with
a block author or a validator. The session keys of accounts are removed once their account balance is zero.
- **Session length:** This pallet does not assume anything about the length of each session.
Rather, it relies on an implementation of `ShouldEndSession` to dictate a new session's start.
This pallet provides the `PeriodicSessions` struct for simple periodic sessions.
- **Session rotation configuration:** Configure as either a 'normal' (rewardable session where rewards are
applied) or 'exceptional' (slashable) session rotation.
- **Session rotation process:** At the beginning of each block, the `on_initialize` function
queries the provided implementation of `ShouldEndSession`. If the session is to end the newly
activated validator IDs and session keys are taken from storage and passed to the
`SessionHandler`. The validator set supplied by `SessionManager::new_session` and the corresponding session
keys, which may have been registered via `set_keys` during the previous session, are written
to storage where they will wait one session before being passed to the `SessionHandler`
themselves.

### Goals

The Session pallet is designed to make the following possible:

- Set session keys of the validator set for upcoming sessions.
- Control the length of sessions.
- Configure and switch between either normal or exceptional session rotations.

## Interface

### Dispatchable Functions

- `set_keys` - Set a validator's session keys for upcoming sessions.

### Public Functions

- `rotate_session` - Change to the next session. Register the new authority set. Queue changes
for next session rotation.
- `disable_index` - Disable a validator by index.
- `disable` - Disable a validator by Validator ID

## Usage

### Example from the FRAME

The [Staking pallet](https://docs.rs/pallet-staking/latest/pallet_staking/) uses the Session pallet to get the validator set.

```rust
use pallet_session as session;

fn validators<T: pallet_session::Config>() -> Vec<<T as pallet_session::Config>::ValidatorId> {
	<pallet_session::Pallet<T>>::validators()
}
```

## Related Modules

- [Staking](https://docs.rs/pallet-staking/latest/pallet_staking/)

License: Apache-2.0
