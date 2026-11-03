We're moving our public status page to Atlassian Statuspage. In Cloudflare, add a DNS record
so that `status.octo-corp.com` is a CNAME to `octocorp.statuspage.io`. Statuspage issues its
own certificate, so the record has to be DNS only, not proxied.

Leave the old `status-old.octo-corp.com` record and everything else as it is.
