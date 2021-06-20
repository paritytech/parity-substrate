# Bounties Module ( pallet-bounties )

## Bounty

**Note :: This pallet is tightly coupled with pallet-treasury**

A Bounty Spending is a reward for a specified body of work - or specified set of objectives - that
needs to be executed for a predefined Treasury amount to be paid out. A curator is assigned after
the bounty is approved and funded by Council, to be delegated with the responsibility of assigning a
payout address once the specified set of objectives is completed.

After the Council has activated a bounty, it delegates the work that requires expertise to a curator
in exchange of a deposit. Once the curator accepts the bounty, they get to close the active bounty.
Closing the active bounty enacts a delayed payout to the payout address, the curator fee and the
return of the curator deposit. The delay allows for intervention through regular democracy. The
Council gets to unassign the curator, resulting in a new curator election. The Council also gets to
cancel the bounty if deemed necessary before assigning a curator or once the bounty is active or
payout is pending, resulting in the slash of the curator's deposit.

### Terminology

- **Bounty spending proposal:** A proposal to reward a predefined body of work upon completion by
  the Treasury.
- **SubBounty:** A large chunk of bounty proposal can be subdivided into small chunks as
  independent subbounties, for parallel execution, minimise the workload on council governance
  & tracking spended funds.
- **Proposer:** An account proposing a bounty spending.
- **Curator or Master Curator or Sub Curator:** An account managing the bounty or subbounty
  and assigning a payout address receiving the reward for the completion of work.
- **Deposit:** The amount held on deposit for placing a bounty proposal plus the amount held on
  deposit per byte within the bounty description.
- **Curator deposit:** The payment from a candidate willing to curate an approved bounty. The
  deposit is returned when/if the bounty is completed.
- **Bounty value:** The total amount that should be paid to the Payout Address if the bounty is
  rewarded.
- **Payout address:** The account to which the total or part of the bounty is assigned to.
- **Payout Delay:** The delay period for which a bounty beneficiary needs to wait before claiming.
- **Curator fee:** The reserved upfront payment for a curator for work related to the bounty.

## Interface

### Dispatchable Functions

Bounty protocol:
- `propose_bounty` - Propose a specific treasury amount to be earmarked for a predefined set of
  tasks and stake the required deposit.
- `approve_bounty` - Accept a specific treasury amount to be earmarked for a predefined body of
  work.
- `propose_curator` - Assign an account to a bounty as candidate curator.
- `accept_curator` - Accept a bounty assignment from the Council, setting a curator deposit.
- `extend_bounty_expiry` - Extend the expiry block number of the bounty and stay active.
- `award_bounty` - Close and pay out the specified amount for the completed work.
- `claim_bounty` - Claim a specific bounty amount from the Payout Address.
- `unassign_curator` - Unassign an accepted curator from a specific earmark.
- `close_bounty` - Cancel the earmark for a specific treasury amount and close the bounty.
- `add_subbounty` - Master curator may split or delegate the execution of bounty,
   by adding a new subbounty, with an amount which can be deducted from the parent bounty.
- `propose_subcurator` - Master curator may assign an account to a subbouty
   as candidate subcurator.
- `accept_subcurator` - Accept a subbounty assignment from the Master curator,
   setting a subcurator deposit.
- `unassign_subcurator` - Unassign an accepted subcurator from a specific earmark.
- `award_subbounty` - Close and specify the subbouty payout beneficiary address.
- `claim_subbounty` - Claim a payout amount & subcurator fee for specific subbounty.
- `close_subbounty` - Cancel the earmark for a specific treasury amount and close the subbounty.
- `extend_subbounty_bounty_expiry` - Extend the expiry time of an bounty of active subbounty.