# Specs

Use this directory for product specs, technical specs, requirements, and acceptance criteria that should be shared through Git.

## Naming

Name specs by module, feature area, or long-lived product surface. Do not prefix spec files with dates.

```text
module-or-feature-name.md
```

Examples:

```text
quota.md
usage-import.md
tui-overview.md
pricing-cache.md
```

The name should stay stable as the feature evolves. Put dated decisions, reviews, and temporary investigation notes in `.agents/notes/` or `.agents/plans/` instead.

## Workflow

For feature work, update or create the relevant spec before implementation planning. Keep implementation details in `.agents/plans/` and research or review notes in `.agents/notes/`.
