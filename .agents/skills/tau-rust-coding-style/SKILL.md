---
name: tau-rust-coding-style
description: >
  Use this skill before writing, reviewing, or refactoring Rust code in Tau,
  especially public APIs, data structures, tests, regression coverage, or
  documentation comments.
user-invocable: true
advertise: true
---

# Tau Rust coding style

## Proper code documentation

Every struct, every public method and all fields in all datastructures MUST have informative docstrings documenting what they do.

Every test must have a docstring explaining: what it is trying to ensure/prevent and justify its existance in a meaningful way.

## Code structure

Rust modules should focus around a single struct.
