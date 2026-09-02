#!/usr/bin/env bash
#
# Provision the idealyst cargo registry: a private S3 bucket behind a
# CloudFront distribution at crates.idealyst.io.
#
# Why CloudFront and not the plain S3 endpoint: the bucket name would otherwise
# be baked into every consumer's .cargo/config.toml, and moving off S3 later
# would strand them. A custom domain keeps the registry's address independent
# of where it is stored.
#
# Why the bucket stays PRIVATE: with an Origin Access Control it is reachable
# only through the distribution, so there is one way in and one place that sets
# cache headers. A public bucket would also serve the index over a second URL
# with different caching — which is how you get "cargo can't see the version I
# just published".
#
# Everything here is idempotent: re-running it converges rather than
# duplicating. Run it with the profile that owns both the bucket account and
# the idealyst.io hosted zone:
#
#   AWS_PROFILE=idealyst ./scripts/provision-registry.sh all
#
# Steps individually: cert | bucket | distribution | dns | status

set -euo pipefail

DOMAIN="${IDEALYST_REGISTRY_DOMAIN:-crates.idealyst.io}"
ZONE_DOMAIN="${DOMAIN#*.}"
BUCKET="${IDEALYST_REGISTRY_BUCKET:-idealyst-crates}"
BUCKET_REGION="${IDEALYST_REGISTRY_REGION:-$(aws configure get region 2>/dev/null || echo us-east-1)}"
# CloudFront reads certificates only from us-east-1, whatever region the bucket
# lives in.
CERT_REGION="us-east-1"
# CloudFront's managed "CachingOptimized" policy. It honours the Cache-Control
# headers `registry publish` sets per object: index files revalidate on every
# request, .crate tarballs cache for a year.
CACHE_POLICY="658327ea-f89d-4fab-a63d-7e88639e58f6"
# CloudFront's zone id is a global constant for alias records.
CF_HOSTED_ZONE="Z2FDTNDATAQYW2"

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
info() { printf '  %s\n' "$*"; }
die()  { printf '\n\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null || die "$1 is not installed"; }
need aws

account_id() { aws sts get-caller-identity --query Account --output text; }

zone_id() {
    aws route53 list-hosted-zones \
        --query "HostedZones[?Name=='${ZONE_DOMAIN}.'].Id | [0]" --output text 2>/dev/null \
        | sed 's|/hostedzone/||'
}

cert_arn() {
    aws acm list-certificates --region "$CERT_REGION" \
        --query "CertificateSummaryList[?DomainName=='${DOMAIN}'].CertificateArn | [0]" \
        --output text 2>/dev/null
}

dist_id() {
    aws cloudfront list-distributions \
        --query "DistributionList.Items[?contains(Aliases.Items || \`[]\`, '${DOMAIN}')].Id | [0]" \
        --output text 2>/dev/null
}

dist_domain() {
    aws cloudfront get-distribution --id "$1" \
        --query Distribution.DomainName --output text
}

# --- certificate ------------------------------------------------------------

cmd_cert() {
    local arn; arn="$(cert_arn)"
    if [ "$arn" = "None" ] || [ -z "$arn" ]; then
        say "requesting a certificate for ${DOMAIN}"
        arn="$(aws acm request-certificate \
            --region "$CERT_REGION" \
            --domain-name "$DOMAIN" \
            --validation-method DNS \
            --query CertificateArn --output text)"
        info "$arn"
        # ACM fills in the validation record asynchronously.
        sleep 8
    else
        say "certificate already exists"
        info "$arn"
    fi

    local status
    status="$(aws acm describe-certificate --region "$CERT_REGION" --certificate-arn "$arn" \
                --query Certificate.Status --output text)"
    if [ "$status" = "ISSUED" ]; then
        info "already ISSUED"
        return 0
    fi

    local zone; zone="$(zone_id)"
    [ "$zone" = "None" ] || [ -z "$zone" ] && die "no Route53 hosted zone for ${ZONE_DOMAIN} in this account"

    say "writing the DNS validation record into ${ZONE_DOMAIN} (zone ${zone})"
    local name value
    name="$(aws acm describe-certificate --region "$CERT_REGION" --certificate-arn "$arn" \
             --query "Certificate.DomainValidationOptions[0].ResourceRecord.Name" --output text)"
    value="$(aws acm describe-certificate --region "$CERT_REGION" --certificate-arn "$arn" \
              --query "Certificate.DomainValidationOptions[0].ResourceRecord.Value" --output text)"
    [ "$name" = "None" ] && die "ACM has not published a validation record yet — re-run in a moment"
    info "$name -> $value"

    aws route53 change-resource-record-sets --hosted-zone-id "$zone" --change-batch "{
      \"Changes\": [{
        \"Action\": \"UPSERT\",
        \"ResourceRecordSet\": {
          \"Name\": \"${name}\",
          \"Type\": \"CNAME\",
          \"TTL\": 300,
          \"ResourceRecords\": [{ \"Value\": \"${value}\" }]
        }
      }]
    }" >/dev/null

    say "waiting for the certificate to be issued (typically 1-3 minutes)"
    aws acm wait certificate-validated --region "$CERT_REGION" --certificate-arn "$arn" \
        && info "ISSUED" \
        || die "certificate did not validate — check the record above"
}

# --- bucket -----------------------------------------------------------------

cmd_bucket() {
    if aws s3api head-bucket --bucket "$BUCKET" 2>/dev/null; then
        say "bucket s3://${BUCKET} already exists"
    else
        say "creating s3://${BUCKET} in ${BUCKET_REGION} (private)"
        if [ "$BUCKET_REGION" = "us-east-1" ]; then
            aws s3api create-bucket --bucket "$BUCKET" --region "$BUCKET_REGION" >/dev/null
        else
            aws s3api create-bucket --bucket "$BUCKET" --region "$BUCKET_REGION" \
                --create-bucket-configuration "LocationConstraint=${BUCKET_REGION}" >/dev/null
        fi
    fi
    # No public access under any circumstance — CloudFront reaches it with a
    # signed request via the origin access control.
    aws s3api put-public-access-block --bucket "$BUCKET" \
        --public-access-block-configuration \
        "BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true"
    info "public access blocked"
}

# --- distribution -----------------------------------------------------------

oac_id() {
    aws cloudfront list-origin-access-controls \
        --query "OriginAccessControlList.Items[?Name=='${BUCKET}-oac'].Id | [0]" \
        --output text 2>/dev/null
}

cmd_distribution() {
    local arn; arn="$(cert_arn)"
    [ "$arn" = "None" ] || [ -z "$arn" ] && die "no certificate — run \`$0 cert\` first"
    local status
    status="$(aws acm describe-certificate --region "$CERT_REGION" --certificate-arn "$arn" \
                --query Certificate.Status --output text)"
    [ "$status" != "ISSUED" ] && die "certificate is ${status}, not ISSUED"

    local existing; existing="$(dist_id)"
    if [ "$existing" != "None" ] && [ -n "$existing" ]; then
        say "distribution already exists"
        info "$existing  ($(dist_domain "$existing"))"
        attach_bucket_policy "$existing"
        return 0
    fi

    local oac; oac="$(oac_id)"
    if [ "$oac" = "None" ] || [ -z "$oac" ]; then
        say "creating the origin access control"
        oac="$(aws cloudfront create-origin-access-control \
            --origin-access-control-config "Name=${BUCKET}-oac,Description=idealyst registry,SigningProtocol=sigv4,SigningBehavior=always,OriginAccessControlOriginType=s3" \
            --query OriginAccessControl.Id --output text)"
    fi
    info "origin access control: ${oac}"

    say "creating the distribution for ${DOMAIN}"
    local cfg; cfg="$(mktemp)"
    trap 'rm -f "$cfg"' RETURN
    cat > "$cfg" <<JSON
{
  "CallerReference": "idealyst-registry-$(date +%s)",
  "Aliases": { "Quantity": 1, "Items": ["${DOMAIN}"] },
  "DefaultRootObject": "",
  "Origins": { "Quantity": 1, "Items": [{
    "Id": "s3",
    "DomainName": "${BUCKET}.s3.${BUCKET_REGION}.amazonaws.com",
    "OriginAccessControlId": "${oac}",
    "OriginPath": "",
    "CustomHeaders": { "Quantity": 0 },
    "S3OriginConfig": { "OriginAccessIdentity": "" }
  }]},
  "DefaultCacheBehavior": {
    "TargetOriginId": "s3",
    "ViewerProtocolPolicy": "redirect-to-https",
    "AllowedMethods": {
      "Quantity": 2, "Items": ["GET","HEAD"],
      "CachedMethods": { "Quantity": 2, "Items": ["GET","HEAD"] }
    },
    "Compress": true,
    "CachePolicyId": "${CACHE_POLICY}"
  },
  "Comment": "idealyst cargo registry",
  "Enabled": true,
  "HttpVersion": "http2and3",
  "PriceClass": "PriceClass_100",
  "ViewerCertificate": {
    "ACMCertificateArn": "${arn}",
    "SSLSupportMethod": "sni-only",
    "MinimumProtocolVersion": "TLSv1.2_2021"
  }
}
JSON
    local id
    id="$(aws cloudfront create-distribution --distribution-config "file://${cfg}" \
            --query Distribution.Id --output text)"
    info "${id}  ($(dist_domain "$id"))"
    attach_bucket_policy "$id"
}

# Let ONLY this distribution read the bucket.
attach_bucket_policy() {
    local id="$1" acct; acct="$(account_id)"
    say "attaching the bucket policy"
    aws s3api put-bucket-policy --bucket "$BUCKET" --policy "{
      \"Version\": \"2012-10-17\",
      \"Statement\": [{
        \"Sid\": \"AllowCloudFrontRead\",
        \"Effect\": \"Allow\",
        \"Principal\": { \"Service\": \"cloudfront.amazonaws.com\" },
        \"Action\": \"s3:GetObject\",
        \"Resource\": \"arn:aws:s3:::${BUCKET}/*\",
        \"Condition\": { \"StringEquals\": {
          \"AWS:SourceArn\": \"arn:aws:cloudfront::${acct}:distribution/${id}\"
        }}
      }]
    }"
    info "only distribution ${id} may read s3://${BUCKET}"
}

# --- dns --------------------------------------------------------------------

cmd_dns() {
    local id; id="$(dist_id)"
    [ "$id" = "None" ] || [ -z "$id" ] && die "no distribution yet — run \`$0 distribution\`"
    local zone; zone="$(zone_id)"
    [ "$zone" = "None" ] || [ -z "$zone" ] && die "no hosted zone for ${ZONE_DOMAIN}"
    local target; target="$(dist_domain "$id")"

    say "pointing ${DOMAIN} at ${target}"
    # An ALIAS, not a CNAME: alias records are free to resolve and can sit at
    # an apex if the registry ever moves to one.
    aws route53 change-resource-record-sets --hosted-zone-id "$zone" --change-batch "{
      \"Changes\": [{
        \"Action\": \"UPSERT\",
        \"ResourceRecordSet\": {
          \"Name\": \"${DOMAIN}\",
          \"Type\": \"A\",
          \"AliasTarget\": {
            \"HostedZoneId\": \"${CF_HOSTED_ZONE}\",
            \"DNSName\": \"${target}\",
            \"EvaluateTargetHealth\": false
          }
        }
      }]
    }" >/dev/null
    info "record written"
}

# --- status -----------------------------------------------------------------

# Serve index.html at the bucket root. A sparse registry has no browsable
# root, so without this `https://crates.idealyst.io/` answers 403 — the bucket
# grants no listing by design — and anyone who pastes the URL into a browser
# gets an access error instead of the setup instructions.
cmd_root() {
    local id; id="$(dist_id)"
    if [ "$id" = "None" ] || [ -z "$id" ]; then die "no distribution yet — run \`$0 distribution\`"; fi

    local tmp; tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' RETURN
    aws cloudfront get-distribution-config --id "$id" > "$tmp/full.json"
    local etag
    etag="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["ETag"])' "$tmp/full.json")"
    # Update in place: CloudFront replaces the WHOLE config, so the existing
    # one has to be round-tripped with just this field changed.
    python3 -c 'import json,sys
full=json.load(open(sys.argv[1]))
cfg=full["DistributionConfig"]
cfg["DefaultRootObject"]="index.html"
json.dump(cfg, open(sys.argv[2],"w"))' "$tmp/full.json" "$tmp/config.json"

    say "setting DefaultRootObject=index.html on ${id}"
    aws cloudfront update-distribution --id "$id" --if-match "$etag" \
        --distribution-config "file://$tmp/config.json" \
        --query "Distribution.Status" --output text | sed 's/^/  /'
}

cmd_status() {
    say "account"; info "$(account_id)  profile=${AWS_PROFILE:-default}"

    say "certificate"
    local arn; arn="$(cert_arn)"
    if [ "$arn" = "None" ] || [ -z "$arn" ]; then
        info "none for ${DOMAIN}"
    else
        info "$(aws acm describe-certificate --region "$CERT_REGION" --certificate-arn "$arn" \
                 --query Certificate.Status --output text)  ${arn}"
    fi

    say "bucket"
    # head-bucket prints a JSON body on success; only its exit code matters.
    aws s3api head-bucket --bucket "$BUCKET" >/dev/null 2>&1 \
        && info "s3://${BUCKET} exists (${BUCKET_REGION})" \
        || info "s3://${BUCKET} does not exist"

    say "distribution"
    local id; id="$(dist_id)"
    if [ "$id" = "None" ] || [ -z "$id" ]; then
        info "none"
    else
        aws cloudfront get-distribution --id "$id" \
            --query "Distribution.{id:Id,domain:DomainName,status:Status}" --output table
    fi

    say "dns"
    local zone; zone="$(zone_id)"
    if [ "$zone" = "None" ] || [ -z "$zone" ]; then
        info "no hosted zone for ${ZONE_DOMAIN} in this account"
    else
        aws route53 list-resource-record-sets --hosted-zone-id "$zone" \
            --query "ResourceRecordSets[?Name=='${DOMAIN}.'].{name:Name,type:Type}" --output text \
            | sed 's/^/  /' || true
    fi

    say "reachable?"
    if curl -sf --max-time 8 "https://${DOMAIN}/index/config.json" >/dev/null 2>&1; then
        info "https://${DOMAIN}/index/config.json is live"
    else
        info "https://${DOMAIN}/index/config.json is not reachable yet"
    fi
}

case "${1:-status}" in
    cert)         cmd_cert ;;
    bucket)       cmd_bucket ;;
    distribution) cmd_distribution ;;
    dns)          cmd_dns ;;
    root)         cmd_root ;;
    all)          cmd_cert; cmd_bucket; cmd_distribution; cmd_dns; cmd_root; cmd_status ;;
    status)       cmd_status ;;
    *) echo "usage: $0 {cert|bucket|distribution|dns|root|all|status}" >&2; exit 1 ;;
esac
