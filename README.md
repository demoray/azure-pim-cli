# Azure PIM CLI

```
CLI to list and enable Azure Privileged Identity Management roles

Usage: az-pim <COMMAND>

Commands:
  list     List eligible assignments
  elevate  Elevate to a specific role

Options:
  -h, --help
          Print help

  -V, --version
          Print version

```
## az-pim list

```
List eligible assignments

Usage: list

Options:
  -h, --help
          Print help

  -V, --version
          Print version

```
## az-pim elevate <ROLE> <SCOPE> <JUSTIFICATION>

```
Elevate to a specific role

Usage: elevate [OPTIONS] <ROLE> <SCOPE> <JUSTIFICATION>

Arguments:
  <ROLE>
          Name of the role to elevate

  <SCOPE>
          Scope to elevate

  <JUSTIFICATION>
          Justification for the request

Options:
      --duration <DURATION>
          Duration in minutes

          [default: 480]

  -h, --help
          Print help

  -V, --version
          Print version

```