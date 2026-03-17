# Jira API Notes (ENG Board)

## Auth (session-only)
Use Basic Auth with email + API token, set via env vars:

```
export JIRA_BASE_URL="https://tribble-ai.atlassian.net"
export JIRA_EMAIL="sunil@tribble.ai"
export JIRA_API_TOKEN="..."
```

Avoid writing tokens to files or git.

## Base Endpoints
- `GET /rest/api/3/project`
- `POST /rest/api/3/search/jql`
- `GET /rest/api/3/issue/{key}`
- `POST /rest/api/3/issue`
- `PUT /rest/api/3/issue/{key}`
- `POST /rest/api/3/issue/{key}/comment`
- `GET /rest/api/3/issue/{key}/transitions`
- `POST /rest/api/3/issue/{key}/transitions`
- `POST /rest/api/3/issue/{key}/attachments` (set `X-Atlassian-Token: no-check`)
- `GET /rest/api/3/field`

## JQL Snippets
- Recent ENG requests: `project = ENG AND created >= -14d ORDER BY created DESC`
- Recently updated: `project = ENG AND updated >= -14d ORDER BY updated DESC`
- Features (if issue type exists): `project = ENG AND issuetype = Feature AND created >= -14d ORDER BY created DESC`
- Features by label: `project = ENG AND labels = feature AND created >= -14d ORDER BY created DESC`

## ADF Helper
Use Atlassian Document Format (ADF) for description/comment if plain text fails.

```
{
  "type": "doc",
  "version": 1,
  "content": [
    {
      "type": "paragraph",
      "content": [
        {"type": "text", "text": "Line 1"}
      ]
    }
  ]
}
```

## Epic Linking
1. List fields and find the custom id for `Epic Link` or `Parent Link`.
2. Set that field to the epic key when creating child issues.

Example:
```
{"fields": {"customfield_12345": "ENG-456"}}
```

## Attachments
Use multipart/form-data and `X-Atlassian-Token: no-check`.
