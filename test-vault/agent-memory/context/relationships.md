---
title: Relationships & Cross-References
type: agent-memory
category: relationships
owner: assistant
last_updated: "2026-03-18"
tags: [links, people, projects, context-web]
---

# Relationships & Cross-References

This document maps the connections between people, projects, documents, and recurring topics. It serves as the agent's "context web" — a quick way to find related information across the memory system.

## People → Projects

| Person | Primary Projects | Notes |
|--------|-----------------|-------|
| [Sam Okafor](../longterm.md#important-people) | [API Gateway Migration](#api-gateway-migration), Platform reliability | Reports to Alex directly. Has strong opinions on gRPC vs REST. |
| [Priya Sharma](../longterm.md#important-people) | [Search Redesign](#search-redesign), Product squad velocity | Recently promoted. Alex is actively mentoring her on stakeholder management. |
| [Dana Reeves](../longterm.md#important-people) | [Roadmap Planning](#roadmap), [Search Redesign](#search-redesign) | Co-owns roadmap with Alex. Prefers async docs over meetings. |
| [Morgan Torres](../longterm.md#important-people) | [Incident Response Overhaul](#incident-response), SRE tooling | External to Alex's org but key collaborator. Monthly check-in. |
| [Riley Kim](../longterm.md#important-people) | Onboarding, small Product squad tickets | Paired with Priya for day-to-day. Alex checks in weekly on Thursdays. |
| [Jordan Park](../longterm.md#important-people) | Executive reviews, [Headcount](#hiring) approvals | Alex's skip-level. Prep quarterly metrics deck by end of each quarter. |
| Lisa Huang | [Hiring](#hiring) | Recruiter. Slack channel: #eng-hiring-product. |

## Project Cross-References

### API Gateway Migration
- **Status:** Phase 2 — traffic shifting (30% migrated as of 2026-03-15)
- **Key docs:** [Architecture RFC](../daily-logs/2026-03-10.md), [Migration runbook](../../runbooks/incident-response.md)
- **Depends on:** Platform squad capacity, SRE sign-off on traffic cutover
- **Blocked by:** Nothing currently. Previous blocker (certificate rotation) resolved [2026-03-07](../daily-logs/2026-03-07.md)
- **Links to:** [Sam's 1:1 notes](../daily-logs/2026-03-14.md#sam-1-1), [Standup themes](../daily-logs/2026-03-17.md#standup)
- **Risk:** If Phase 2 slips past April, it collides with performance cycle prep

### Search Redesign
- **Status:** Discovery phase. Priya leading technical spike.
- **Key docs:** [Product brief from Dana](../daily-logs/2026-03-12.md#search-redesign-brief), [Priya's spike notes](../daily-logs/2026-03-15.md#search-spike)
- **Depends on:** User research results (due 2026-03-25), Product squad bandwidth after current sprint
- **Links to:** [Dana weekly sync](../daily-logs/2026-03-11.md#dana-sync), [Priya 1:1](../daily-logs/2026-03-13.md#priya-1-1)
- **Open question:** Build vs buy for the relevance scoring layer. Alex leans build, Dana leans buy.

### Incident Response
- **Status:** Draft phase. New runbook template in review.
- **Key docs:** [Kickoff notes](../daily-logs/2026-03-03.md#incident-response-kickoff), [Morgan's proposed rotation](../daily-logs/2026-03-10.md#morgan-rotation)
- **Depends on:** SRE team buy-in, tooling budget approval from Jordan
- **Links to:** [Morgan check-in](../daily-logs/2026-03-10.md#morgan-checkin), [longterm initiatives](../longterm.md#ongoing-initiatives)

### Roadmap
- **Status:** Q2 planning in progress
- **Key docs:** [Q1 retro](../daily-logs/2026-03-05.md#q1-retro), [Q2 priorities draft](../daily-logs/2026-03-14.md#q2-priorities)
- **Links to:** [Dana sync](../daily-logs/2026-03-11.md#dana-sync), [Jordan prep](../daily-logs/2026-03-17.md#jordan-prep)
- **Decision needed:** Whether to commit to Search Redesign in Q2 or defer to Q3

### Hiring
- **Status:** 4 candidates in interview stage for 2 senior eng roles (Product squad)
- **Links to:** [Interview debrief](../daily-logs/2026-03-13.md#interview-debrief), [Headcount justification](../daily-logs/2026-03-06.md#headcount-doc)
- **Key contact:** Lisa Huang (recruiter), #eng-hiring-product Slack channel
- **Timeline:** Offers out by end of March if candidates clear final round

## Recurring Meeting Map

| Meeting | Cadence | Key People | Typical Topics | Related Logs |
|---------|---------|------------|----------------|--------------|
| Team standup | Daily 8:15 AM | All 12 reports | Blockers, sprint progress | Every [daily log](../daily-logs/) |
| Dana weekly sync | Tue 2 PM | Alex, Dana | Roadmap, priorities, cross-squad | [2026-03-11](../daily-logs/2026-03-11.md), [2026-03-04](../daily-logs/2026-03-04.md) |
| Sam 1:1 | Thu 10 AM | Alex, Sam | Platform squad, gateway migration | [2026-03-14](../daily-logs/2026-03-14.md), [2026-03-07](../daily-logs/2026-03-07.md) |
| Priya 1:1 | Thu 11 AM | Alex, Priya | Product squad, mentoring | [2026-03-13](../daily-logs/2026-03-13.md), [2026-03-06](../daily-logs/2026-03-06.md) |
| Riley check-in | Thu 3 PM | Alex, Riley | Onboarding, growth | [2026-03-14](../daily-logs/2026-03-14.md), [2026-03-07](../daily-logs/2026-03-07.md) |
| Jordan 1:1 | Bi-weekly Fri 9 AM | Alex, Jordan | Team health, initiatives, headcount | [2026-03-15](../daily-logs/2026-03-15.md), [2026-03-01](../daily-logs/2026-03-01.md) |
| Morgan check-in | Monthly | Alex, Morgan | Incident response, SRE partnership | [2026-03-10](../daily-logs/2026-03-10.md) |
| All-hands | Monthly (1st Wed) | Whole company | Company updates | [2026-03-05](../daily-logs/2026-03-05.md) |

## Topic → Memory Links

- **Performance reviews** → [longterm.md](../longterm.md#ongoing-initiatives) (cycle opens 2026-04-15), [2026-03-17 prep notes](../daily-logs/2026-03-17.md#perf-prep)
- **Team morale** → [2026-03-12 retro themes](../daily-logs/2026-03-12.md#retro-themes), [2026-03-05 all-hands](../daily-logs/2026-03-05.md#all-hands)
- **Technical debt** → [Sam's tech debt inventory](../daily-logs/2026-03-14.md#tech-debt), [Q2 priorities](../daily-logs/2026-03-14.md#q2-priorities)
- **On-call** → [Incident response](#incident-response), [Morgan's rotation proposal](../daily-logs/2026-03-10.md#morgan-rotation)
- **Half marathon training** → [longterm.md](../longterm.md#personal-notes) (May race), occasionally affects end-of-day availability
