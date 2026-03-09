# EA Scenario Battery

This is the working battery for proving that Linus is becoming a strong executive assistant rather than only a code harness.

Each scenario should exercise:
- memory and inbox recall
- stateful follow-up across multiple turns
- intelligent inference before questioning
- explicit delegation paths
- approval-gated external actions
- recovery when a connector, quota, or human dependency blocks progress

## Core Scenarios

1. Parent travel orchestration
- Infer who the parents are from memory/inbox.
- Infer likely destination, home airport, and constraints.
- Ask only the high-signal missing questions.
- Produce ranked options, costs, and a handoff path to Rhaine when approved.

2. Doctor appointment scheduling
- Infer specialty, provider history, referral context, and insurance clues.
- Decide whether the best next action is email, portal, Rhaine, or a direct phone call.
- Generate the exact call script and questions needed before acting.

3. Restaurant reservation by phone
- Infer likely restaurant, party size, date window, dietary constraints, and whether this is business or personal.
- Use `phone_call` only after approval.
- Success proof is a real reservation or a confirmation email/SMS captured back into the assistant flow.

4. Home-service or contractor coordination
- Research options, compare quotes/availability, and route logistics to Rhaine when appropriate.
- Track waiting-fors and follow-ups instead of stopping at “here are some options.”

5. Vendor / partner escalation
- Read the thread history, understand urgency and stakeholders, prepare a response draft, and decide whether Slack/email/call is the correct action path.

6. Guest logistics and executive hosting
- Combine travel, calendar, restaurant, and house constraints in one plan.
- Surface conflicts proactively and delegate sub-work instead of serializing everything through the main thread.

7. Tweet or market-signal to execution
- Receive a tweet, memo, or market claim and turn it into concrete repo work.
- Audit what is actually true in the codebase, what is hype, and what should be implemented next.
- Propose owner split across Linus, subLinus, and humans, plus an ongoing background research loop.

## Adjacent High-Agency Variants

1. Vendor capability announcement
- Read a new product launch post, compare it to current stack reality, and produce a pilot or rejection memo with implementation steps.

2. Customer escalation into action
- Read an unhappy customer thread, inspect product/docs, draft the response, and coordinate engineering + EA follow-up.

3. Investor or partner diligence request
- Pull the supporting evidence, draft the response packet, and route missing materials to the right owner.

4. Medical scheduling with physical-world execution
- Infer the specialty and constraints, then use phone/email/human delegation in the correct order until the appointment is actually secured.

5. Local reservation or concierge task
- Infer context from calendar, contacts, and prior threads, then use calling or delegation to land an external confirmation artifact.

## Execution Rules

- The assistant should not overfit to these exact prompts.
- Each scenario should have adjacent variants with different domains, contacts, and urgency levels.
- Live-action cases should stop at approval by default.
- Once the call stack is stable, the first end-to-end live-action proof should be a restaurant reservation that results in an external confirmation artifact.
