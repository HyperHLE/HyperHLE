# AGENTS.md

## Project

fuckingHLEforCoolGames is an independent fork of HyperHLE, itself derived from touchHLE.

The project aims to improve compatibility with older iOS applications and games, with particular attention to ARMv7 applications, Unity-based games, graphics compatibility, and missing platform APIs.

## Development Guidelines

* Preserve the existing architecture unless there is a clear reason to change it.
* Prefer small, focused changes over large rewrites.
* Keep compatibility with existing supported applications.
* Avoid adding application-specific hacks when a general implementation is possible.
* Do not remove existing compatibility behavior without testing its impact.
* Follow the coding style already used in the affected source files.
* Keep platform-specific code isolated where practical.
* Do not introduce unnecessary dependencies.

## Compatibility

Compatibility work should prioritize general iOS behavior over individual games.

When implementing a missing API:

1. Determine how the original iOS API behaves.
2. Check how existing HyperHLE/touchHLE code handles related functionality.
3. Implement the smallest compatible behavior needed.
4. Test against the affected application.
5. Check that existing applications still behave correctly.

Application-specific workarounds should only be used when reproducing the original behavior is impractical or when the workaround is clearly isolated.

## Unity

Unity applications are a major compatibility target.

Older Unity versions may depend on:

* Mono
* UnityEngine assemblies
* Assembly-CSharp assemblies
* UnityScript and Boo
* OpenGL ES 1.x or 2.0
* Unity asset files
* Native plugins
* iOS-specific APIs

Do not assume that all Unity applications use the same runtime behavior.

When debugging a Unity application, first determine:

* Unity version
* CPU architecture
* Graphics API
* Managed assemblies present
* Native libraries loaded
* Required application data
* The point at which execution stops

## Debugging

Runtime logs are important compatibility evidence.

When investigating a crash or early exit:

* Identify the last successful operation.
* Determine whether the application, runtime, or host requested termination.
* Do not assume that `exit(1)` necessarily indicates a host crash.
* Preserve the original log context when possible.
* Add diagnostic logging before changing behavior.
* Prefer obtaining a call site, program counter, backtrace, or equivalent evidence over guessing.

For example, distinguish between:

```text
App called exit(1)
```

and:

```text
User requested quit, exiting.
```

These represent different execution paths and should not be treated as the same failure.

## Graphics

Graphics compatibility should be implemented through the existing graphics abstraction whenever possible.

Avoid hard-coding host GPU behavior for individual applications.

When debugging rendering problems, record:

* Requested OpenGL ES version
* Host graphics API/version
* Renderer and vendor
* Relevant extensions
* First failing graphics operation
* Application-reported renderer information

## File and Bundle Handling

Applications should be treated as immutable whenever possible.

Prefer fixing path resolution, bundle handling, API behavior, or runtime compatibility rather than modifying the original application.

Pay particular attention to:

* `.app` bundles
* `Data/Managed`
* Unity data archives
* Native libraries
* Frameworks
* Resource paths
* Case-sensitive path behavior

## Performance

Do not sacrifice compatibility for premature optimization.

When startup or loading is slow, identify the expensive operation before optimizing it.

Avoid adding broad caching or skipping initialization solely because it improves startup time unless the behavior has been verified to remain compatible.

## Changes

Before committing a change:

* Build the affected target.
* Test the relevant application.
* Check the runtime log for new warnings or errors.
* Avoid unrelated formatting changes.
* Keep commits focused and descriptive.

## Documentation

Documentation should describe actual behavior rather than intended behavior.

Do not claim an application is fully supported when it only launches.

Useful compatibility states include:

* Does not launch
* Loads partially
* Reaches main menu
* Renders correctly
* Playable
* Fully tested

## AI-Assisted Development

AI assistance may be used during development, debugging, code review, and research.

AI-generated changes must still be reviewed and tested before being committed.

Do not accept generated code solely because it appears plausible. Verify API behavior, memory handling, threading, platform assumptions, and compatibility with the existing architecture.

## Scope

Keep the project focused on compatibility and runtime development.

Do not introduce unrelated features unless they directly support the project's goals.
