# Azure PIM CLI

CLI to list and enable Azure Privileged Identity Management roles

```
Usage: az-pim <COMMAND>

Commands:
  list          List eligible assignments
  activate      Activate a specific role
  activate-set  Activate a set of roles

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
## az-pim activate <ROLE> <SCOPE> <JUSTIFICATION>

```
Activate a specific role

Usage: activate [OPTIONS] <ROLE> <SCOPE> <JUSTIFICATION>

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
## az-pim activate-set <JUSTIFICATION>

```
Activate a set of roles

This command can be used to activate multiple roles at once.  It can be used with a config file or by specifying roles on the command line.

Usage: activate-set [OPTIONS] <JUSTIFICATION>

Arguments:
  <JUSTIFICATION>
          Justification for the request

Options:
      --duration <DURATION>
          Duration in minutes

          [default: 480]

      --config <CONFIG>
          Path to a JSON config file containing a set of roles to elevate

          Example config file: ` [ { "scope": "/subscriptions/00000000-0000-0000-0000-000000000000", "role": "Owner" }, { "scope": "/subscriptions/00000000-0000-0000-0000-000000000001", "role": "Owner" } ] `

      --role <SCOPE=NAME>
          Specify a role to elevate

          Specify multiple times to include multiple key/value pairs

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version

```