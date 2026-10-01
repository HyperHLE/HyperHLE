# fuckingHLEforCoolGames

An independent fork of [HyperHLE](https://github.com/KlugKlugTG/HyperHLE) focused on expanding compatibility with older iOS games and applications.

The project builds upon HyperHLE's existing compatibility work while experimenting with additional engines, game-specific fixes, and improvements to the runtime environment.

## Features

* ARMv7 iOS application support
* OpenGL ES compatibility layers
* Unity game support
* Support for multiple application frameworks and engines
* Improved full-screen and display handling
* Game-specific compatibility fixes
* Runtime diagnostics and debugging improvements
* Community-driven development

## Compatibility

Compatibility varies considerably between applications.

The primary goal is to run software that was originally designed for older iOS devices, particularly applications using ARMv7 and older versions of iOS.

Unity-based applications are an important area of development, with testing currently including games built with older Unity versions.

A successful launch does not necessarily mean that an application is fully playable. Missing system APIs, networking services, graphics features, or other platform-specific behavior may still prevent a game from functioning correctly.

## Building

Build instructions depend on the target platform and are documented in the project documentation.

The project currently targets desktop platforms supported by its underlying HyperHLE architecture.

## Development

Development focuses on improving compatibility without requiring modifications to the original applications whenever possible.

When investigating compatibility problems, runtime logs and reproducible test cases are preferred over application-specific workarounds.

## Project Status

This project is experimental and under active development.

Many applications will not work correctly yet. Compatibility can change between revisions as additional APIs and platform behavior are implemented.

## Relationship to HyperHLE

fuckingHLEforCoolGames is an independent fork of HyperHLE.

Changes made in this project may diverge substantially from upstream as new compatibility work is developed.

Upstream HyperHLE remains an important reference for the project.

## Contributing

Bug reports, compatibility information, test results, and code contributions are welcome.

When reporting an application compatibility issue, include:

* Application name and version
* iOS version the application originally targeted, if known
* Device architecture
* Relevant runtime logs
* Description of the observed behavior

## License

See the `LICENSE` file for the applicable license information.
