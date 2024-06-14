# Azure PIM CLI

Unofficial CLI to list and enable Azure Privileged Identity Management (PIM) roles

```
Usage: az-pim <COMMAND>

Commands:
  list          List eligible assignments
  activate      Activate a specific role
  activate-set  Activate a set of roles
  interactive   Activate roles interactively
  init          Setup shell tab completions

Options:
  -h, --help
          Print help

```
## az-pim list

```
List eligible assignments

Usage: list

Options:
  -h, --help
          Print help

```
### Example Usage

```
$ az-pim list
[
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "scope_name": "contoso-development",
  },
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000001",
    "scope_name": "contoso-development-2",
  }
]
$
```

## az-pim activate <ROLE> <SCOPE> <JUSTIFICATION>

```
Activate a specific role

Example usage:
```

```
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
          Print help (see a summary with '-h')

```
### Example Usage

```
$ az-pim activate "Storage Blob Data Contributor" "/subscriptions/00000000-0000-0000-0000-000000000000" "accessing storage data"
2024-06-04T15:35:50.330623Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development
$ az-pim activate "Storage Blob Data Contributor" "contoso-development-2" "accessing storage data"
2024-06-04T15:35:54.714131Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development-2
$
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

          Example config file: ` [ { "role": "Owner", "scope": "/subscriptions/00000000-0000-0000-0000-000000000000" }, { "role": "Owner", "scope": "/subscriptions/00000000-0000-0000-0000-000000000001" } ] `

      --role <ROLE=SCOPE>
          Specify a role to elevate

          Specify multiple times to include multiple key/value pairs

  -h, --help
          Print help (see a summary with '-h')

```
### Example Usage

```
$ # specifying multiple roles using a configuration file
$ az-pim activate-set "deploying new code" --config roles.json
2024-06-04T15:22:03.1051Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development
2024-06-04T15:22:07.25Z    INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development-2
$ cat roles.json
[
  {
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "role": "Storage Blob Data Contributor"
  },
  {
    "scope": "contoso-development-2",
    "role": "Storage Blob Data Contributor"
  }
]
$ # specifying multiple roles via the command line
$ az-pim activate-set "deploying new code" --role "Storage Blob Data Contributor=/subscriptions/00000000-0000-0000-0000-000000000000" --role "Storage Blob Data Contributor=contoso-development-2"
2024-06-04T15:21:39.9341Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development
2024-06-04T15:21:43.1522Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development-2
$ # use `jq` to select roles to activate from the current role assignments
$ az-pim list | jq 'map(select(.role | contains("Contributor")))' | az-pim activate-set "deploying new code" --config /dev/stdin
2024-06-04T18:47:15.489917Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development
2024-06-04T18:47:20.510941Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development-2
$
```

## az-pim interactive

```
Activate roles interactively

Usage: interactive [OPTIONS]

Options:
      --justification <JUSTIFICATION>
          Justification for the request

  -h, --help
          Print help

```
## az-pim init <SHELL>

```
Setup shell tab completions

This command will generate shell completions for the specified shell.

Usage: init <SHELL>

Arguments:
  <SHELL>
          [possible values: bash, elvish, fish, powershell, zsh]

Options:
  -h, --help
          Print help (see a summary with '-h')

```
### Example Usage

```
* bash: `eval $(az-pim init bash)`
* zsh: `source <(az-pim init zsh)`
```
