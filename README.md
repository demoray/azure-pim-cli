# Azure PIM CLI

Unofficial CLI to list and enable Azure Privileged Identity Management (PIM) roles

```
Usage: az-pim [OPTIONS] <COMMAND>

Commands:
  list        List active or eligible assignments
  activate    Activate roles
  deactivate  Deactivate roles
  role        Manage Azure role-based access control (Azure RBAC)
  init        Setup shell tab completions

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

  -V, --version
          Print version

```
## az-pim list

```
List active or eligible assignments

Usage: list [OPTIONS]

Options:
      --active
          List active assignments

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

```
### Example Usage

```
$ az-pim list
[
  {
    "role": "Owner",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "scope_name": "My Subscription"
  },
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "scope_name": "My Subscription"
  }
]
$ az-pim list --active
[
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "scope_name": "My Subscription"
  }
]
$
```

## az-pim activate

```
Activate roles

Usage: activate [OPTIONS] <COMMAND>

Commands:
  role         Activate a specific role
  set          Activate a set of roles
  interactive  Activate roles interactively

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

```
### az-pim activate role <ROLE> <SCOPE> <JUSTIFICATION>

```
Activate a specific role

Usage: role [OPTIONS] <ROLE> <SCOPE> <JUSTIFICATION>

Arguments:
  <ROLE>
          Name of the role to activate

  <SCOPE>
          Scope to activate

  <JUSTIFICATION>
          Justification for the request

Options:
      --duration <DURATION>
          Duration for the role to be active

          Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'

          [default: "8 hours"]

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

      --wait <WAIT>
          Duration to wait for the roles to be activated

          Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'

  -h, --help
          Print help (see a summary with '-h')

```
#### Example Usage

```
$ az-pim activate role Owner "My Subscription" "developing pim"
2024-06-27T16:55:27.676291Z  INFO az_pim: activating Owner in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$
```

### az-pim activate set <JUSTIFICATION>

```
Activate a set of roles

This command can be used to activate multiple roles at once.  It can be used with a config file or by specifying roles on the command line.

Usage: set [OPTIONS] <JUSTIFICATION>

Arguments:
  <JUSTIFICATION>
          Justification for the request

Options:
      --duration <DURATION>
          Duration for the role to be active

          Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'

          [default: "8 hours"]

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --config <CONFIG>
          Path to a JSON config file containing a set of roles to activate

          Example config file: ` [ { "role": "Owner", "scope": "/subscriptions/00000000-0000-0000-0000-000000000000" }, { "role": "Owner", "scope": "/subscriptions/00000000-0000-0000-0000-000000000001" } ] `

      --quiet
          Only show errors

      --role <ROLE=SCOPE>
          Specify a role to activate

          Specify multiple times to include multiple key/value pairs

      --concurrency <CONCURRENCY>
          Concurrency rate

          Specify how many roles to activate concurrently.  This can be used to speed up activation of roles.

          [default: 4]

      --wait <WAIT>
          Duration to wait for the roles to be activated

          Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'

  -h, --help
          Print help (see a summary with '-h')

```
#### Example Usage

```
$ az-pim activate set 'continued development' --role 'Owner=My Subscription'
2024-06-27T17:23:03.981067Z  INFO azure_pim_cli: activating Owner in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$ cat config.json
[
  {
    "role": "Owner",
    "scope_name": "My Subscription"
  },
  {
    "role": "Storage Blob Data Contributor",
    "scope_name": "My Subscription"
  }
]
$ az-pim activate set 'continued development' --config ./config.json
2024-06-27T17:23:03.981067Z  INFO azure_pim_cli: activating Owner in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
2024-06-27T17:23:03.981067Z  INFO azure_pim_cli: activating Storabe Blob Data Contributor in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$ az-pim list | jq 'map(select(.role | contains("Contributor")))' | az-pim activate set "deploying new code" --config /dev/stdin
2024-06-27T17:23:03.981067Z  INFO azure_pim_cli: activating Storabe Blob Data Contributor in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$
```

### az-pim activate interactive

```
Activate roles interactively

Usage: interactive [OPTIONS]

Options:
      --justification <JUSTIFICATION>
          Justification for the request

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --concurrency <CONCURRENCY>
          Concurrency rate

          Specify how many roles to activate concurrently.  This can be used to speed up activation of roles.

          [default: 4]

      --quiet
          Only show errors

      --duration <DURATION>
          Duration for the role to be active

          Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'

          [default: "8 hours"]

      --wait <WAIT>
          Duration to wait for the roles to be activated

          Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'

  -h, --help
          Print help (see a summary with '-h')

```
## az-pim deactivate

```
Deactivate roles

Usage: deactivate [OPTIONS] <COMMAND>

Commands:
  role         Deactivate a specific role
  set          Deactivate a set of roles
  interactive  Deactivate roles interactively

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

```
### az-pim deactivate role <ROLE> <SCOPE>

```
Deactivate a specific role

Usage: role [OPTIONS] <ROLE> <SCOPE>

Arguments:
  <ROLE>
          Name of the role to deactivate

  <SCOPE>
          Scope to deactivate

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

```
#### Example Usage

```
$ az-pim deactivate role "Storage Queue Data Contributor" "My Subscription"
2024-06-27T17:57:53.462674Z  INFO az_pim: deactivating Storage Queue Data Contributor in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$
```

### az-pim deactivate set

```
Deactivate a set of roles

Usage: set [OPTIONS]

Options:
      --config <CONFIG>
          Path to a JSON config file containing a set of roles to deactivate

          Example config file: ` [ { "role": "Owner", "scope": "/subscriptions/00000000-0000-0000-0000-000000000000" }, { "role": "Owner", "scope": "/subscriptions/00000000-0000-0000-0000-000000000001" } ] `

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

      --role <ROLE=SCOPE>
          Specify a role to deactivate

          Specify multiple times to include multiple key/value pairs

      --concurrency <CONCURRENCY>
          Concurrency rate

          Specify how many roles to deactivate concurrently.  This can be used to speed up activation of roles.

          [default: 4]

  -h, --help
          Print help (see a summary with '-h')

```
#### Example Usage

```
$ az-pim deactivate set --role "Owner=My Subscription"
2024-06-27T17:57:53.462674Z  INFO az_pim: deactivating Owner in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$ # deactivate all roles by listing active roles, then deactivating all of them
$ az-pim list | az-pim deactivate set --config /dev/stdin
2024-06-27T17:57:53.462674Z  INFO az_pim: deactivating Storage Blob Data Contributor in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$
```

### az-pim deactivate interactive

```
Deactivate roles interactively

Usage: interactive [OPTIONS]

Options:
      --concurrency <CONCURRENCY>
          Concurrency rate

          Specify how many roles to deactivate concurrently.  This can be used to speed up deactivation of roles.

          [default: 4]

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help (see a summary with '-h')

```
## az-pim role

```
Manage Azure role-based access control (Azure RBAC)

Usage: role [OPTIONS] <COMMAND>

Commands:
  assignment  Manage role assignments
  definition  Manage role definitions

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

```
### az-pim role assignment

```
Manage role assignments

Usage: assignment [OPTIONS] <COMMAND>

Commands:
  list                   List assignments
  delete                 Delete an assignment
  delete-set             Delete a set of assignments
  delete-orphan-entries  Delete assignments that objects in Microsoft Graph cannot be found

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

```
#### az-pim role assignment list

```
List assignments

Usage: list [OPTIONS]

Options:
      --subscription <SUBSCRIPTION>
          Subscription

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

      --resource-group <RESOURCE_GROUP>
          Resource Group

          This argument requires `subscription` to be set.

      --provider <PROVIDER>
          Provider

          This argument requires `subscription` and `resource_group` to be set.

      --scope <SCOPE>
          Specify scope directly

  -h, --help
          Print help (see a summary with '-h')

```
##### Example Usage

```
$ az-pim role assignment list --subscription 00000000-0000-0000-0000-000000000000
[
  {
    "id": "/subscriptions/00000000-0000-0000-0000-000000000000/resourcegroups/my-resource-group/providers/Microsoft.Storage/storageAccounts/mystorageaccount/providers/Microsoft.Authorization/roleAssignments/00000000-0000-0000-0000-000000000001",
    "name": "00000000-0000-0000-0000-000000000001",
    "properties": {
      "createdOn": "2024-07-03T17:06:36.5812308Z",
      "createdBy": "00000000-0000-0000-0000-000000000002",
      "updatedOn": "2024-07-03T17:06:36.5812308Z",
      "updatedBy": "00000000-0000-0000-0000-000000000003",
      "roleDefinitionId": "/subscriptions/00000000-0000-0000-0000-000000000000/providers/Microsoft.Authorization/roleDefinitions/00000000-0000-0000-0000-000000000004",
      "principalId": "00000000-0000-0000-0000-000000000005",
      "principalType": "ServicePrincipal",
      "scope": "/subscriptions/00000000-0000-0000-0000-000000000000/resourcegroups/my-resource-group/providers/Microsoft.Storage/storageAccounts/mystorageaccount"
    },
    "type": "Microsoft.Authorization/roleAssignments"
  }
]
$
```

#### az-pim role assignment delete <ASSIGNMENT_NAME>

```
Delete an assignment

Usage: delete [OPTIONS] <ASSIGNMENT_NAME>

Arguments:
  <ASSIGNMENT_NAME>
          Assignment name

Options:
      --subscription <SUBSCRIPTION>
          Subscription

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

      --resource-group <RESOURCE_GROUP>
          Resource Group

          This argument requires `subscription` to be set.

      --provider <PROVIDER>
          Provider

          This argument requires `subscription` and `resource_group` to be set.

      --scope <SCOPE>
          Specify scope directly

  -h, --help
          Print help (see a summary with '-h')

```
##### Example Usage

```
$ az-pim role assignment delete 00000000-0000-0000-0000-000000000000 --subscription 00000000-0000-0000-0000-000000000001
$
```

#### az-pim role assignment delete-set <CONFIG>

```
Delete a set of assignments

Usage: delete-set [OPTIONS] <CONFIG>

Arguments:
  <CONFIG>
          Path to a JSON config file containing a set of assignments to delete

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

```
##### Example Usage

```
$ az-pim role assignment list --subscription 00000000-0000-0000-0000-000000000000 | jq 'map(select(.object | .==null)) [].id' | az-pim role assignment delete-set /dev/stdin
2024-07-09T18:54:48.903483Z  INFO azure_pim_cli: listing assignments assignments
2024-07-09T18:19:32.222267Z  INFO azure_pim_cli: deleting assignment 00000000-0000-0000-0000-000000000001 from /subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/my-resource-group/providers/Microsoft.Storage/storageAccounts/mystorageaccount
2024-07-09T18:19:32.222267Z  INFO azure_pim_cli: deleting assignment 00000000-0000-0000-0000-000000000002 from /subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/my-resource-group/providers/Microsoft.Storage/storageAccounts/mystorageaccount
$
```

#### az-pim role assignment delete-orphan-entries

```
Delete assignments that objects in Microsoft Graph cannot be found

Usage: delete-orphan-entries [OPTIONS]

Options:
      --subscription <SUBSCRIPTION>
          Subscription

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

      --resource-group <RESOURCE_GROUP>
          Resource Group

          This argument requires `subscription` to be set.

      --provider <PROVIDER>
          Provider

          This argument requires `subscription` and `resource_group` to be set.

      --scope <SCOPE>
          Specify scope directly

      --yes
          Always respond yes to confirmations

  -h, --help
          Print help (see a summary with '-h')

```
##### Example Usage

```
$ az-pim role assignment delete-orphan-entries --subscription 00000000-0000-0000-0000-000000000001
2024-07-09T18:00:48.903483Z  INFO azure_pim_cli: listing assignments
2024-07-09T18:00:49.066017Z  INFO az_pim: Are you sure you want to delete role 558e61ce-6981-5b70-afc0-c9ca9cdccd8f? (y/n):
y
2024-07-09T18:00:51.222267Z  INFO azure_pim_cli: deleting assignment 00000000-0000-0000-0000-000000000001 from /subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/my-resource-group/providers/Microsoft.Storage/storageAccounts/mystorageaccount
2024-07-09T18:00:52.797752Z  INFO az_pim: Are you sure you want to delete role 1f366ebb-8f4a-524c-8668-02b6d76fdb17? (y/n):
y
2024-07-09T18:00:53.222267Z  INFO azure_pim_cli: deleting assignment 00000000-0000-0000-0000-000000000002 from /subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/my-resource-group/providers/Microsoft.Storage/storageAccounts/mystorageaccount
$
```

### az-pim role definition

```
Manage role definitions

Usage: definition [OPTIONS] <COMMAND>

Commands:
  list  List the definitions for the specific scope

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help

```
#### az-pim role definition list

```
List the definitions for the specific scope

Usage: list [OPTIONS]

Options:
      --subscription <SUBSCRIPTION>
          Limit the scope by the specified Subscription

      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

      --resource-group <RESOURCE_GROUP>
          Limit the scope by the specified Resource Group

          This argument requires `subscription` to be set.

      --provider <PROVIDER>
          Provider

          This argument requires `subscription` and `resource_group` to be set.

      --scope <SCOPE>
          Specify scope directly

  -h, --help
          Print help (see a summary with '-h')

```
##### Example Usage

```
$ az-pim role definition list --subscription 00000000-0000-0000-0000-000000000000
[
  {
    "id": "/subscriptions/00000000-0000-0000-0000-000000000000/providers/Microsoft.Authorization/roleDefinitions/00000000-0000-0000-0000-000000000001",
    "name": "00000000-0000-0000-0000-000000000001",
    "properties": {
      "assignableScopes": [
        "/"
      ],
      "createdOn": "2018-11-29T18:46:55.0492387Z",
      "updatedOn": "2018-11-29T18:46:55.0492387Z",
      "description": "my custom role",
      "permissions": [
        {
          "actions": [
            "Microsoft.Compute/*/read",
            "Microsoft.Network/*/read"
          ],
          "notActions": [],
          "dataActions": [],
          "notDataActions": []
        }
      ],
      "roleName": "my custom name",
      "type": "CustomRole"
    },
    "type": "Microsoft.Authorization/roleDefinitions"
  },
  {
    "id": "/subscriptions/00000000-0000-0000-0000-000000000000/providers/Microsoft.Authorization/roleDefinitions/00000000-0000-0000-0000-000000000007",
    "name": "00000000-0000-0000-0000-000000000007",
    "properties": {
      "assignableScopes": [
        "/"
      ],
      "createdOn": "2017-12-21T00:01:24.7972312Z",
      "updatedOn": "2021-11-11T20:13:54.9397456Z",
      "description": "Allows for read, write and delete access to Azure Storage blob containers and data",
      "permissions": [
        {
          "actions": [
            "Microsoft.Storage/storageAccounts/blobServices/containers/delete",
            "Microsoft.Storage/storageAccounts/blobServices/containers/read",
            "Microsoft.Storage/storageAccounts/blobServices/containers/write",
            "Microsoft.Storage/storageAccounts/blobServices/generateUserDelegationKey/action"
          ],
          "notActions": [],
          "dataActions": [
            "Microsoft.Storage/storageAccounts/blobServices/containers/blobs/delete",
            "Microsoft.Storage/storageAccounts/blobServices/containers/blobs/read",
            "Microsoft.Storage/storageAccounts/blobServices/containers/blobs/write",
            "Microsoft.Storage/storageAccounts/blobServices/containers/blobs/move/action",
            "Microsoft.Storage/storageAccounts/blobServices/containers/blobs/add/action"
          ],
          "notDataActions": []
        }
      ],
      "roleName": "Storage Blob Data Contributor",
      "type": "BuiltInRole"
    },
    "type": "Microsoft.Authorization/roleDefinitions"
  }
]
$
```

## az-pim init <SHELL>

```
Setup shell tab completions

This command will generate shell completions for the specified shell.

Usage: init [OPTIONS] <SHELL>

Arguments:
  <SHELL>
          [possible values: bash, elvish, fish, powershell, zsh]

Options:
      --verbose...
          Increase logging verbosity.  Provide repeatedly to increase the verbosity

      --quiet
          Only show errors

  -h, --help
          Print help (see a summary with '-h')

```
### Example Usage

```
$ # In bash shell
$ eval $(az-pim init bash)
$ # In zsh shell
$ source <(az-pim init zsh)
```
