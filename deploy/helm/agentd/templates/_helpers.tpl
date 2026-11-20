{{- define "agentd.labels" -}}
app.kubernetes.io/name: agentd
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{- end -}}

{{- define "agentd.securityContext" -}}
allowPrivilegeEscalation: false
readOnlyRootFilesystem: true
runAsNonRoot: true
capabilities:
  drop: [ALL]
seccompProfile:
  type: RuntimeDefault
{{- end -}}

{{- define "agentd.databaseUrl" -}}
- name: DATABASE_URL
  valueFrom:
    secretKeyRef:
      name: {{ .Values.database.existingSecret }}
      key: {{ .Values.database.urlKey }}
{{- end -}}

{{- /* An Argo Rollouts canary. Without canary.enabled it has no steps and replaces pods as a rolling update
does; with it, each step sets the canary's weight (by pod count, or by traffic for the api when
trafficRouting is set) or pauses. */ -}}
{{- define "agentd.rolloutStrategy" -}}
{{- $canary := .Values.argoRollouts.canary -}}
strategy:
  canary:
    maxSurge: {{ $canary.maxSurge | quote }}
    maxUnavailable: {{ $canary.maxUnavailable }}
    {{- if $canary.enabled }}
    steps:
      {{- toYaml $canary.steps | nindent 6 }}
    {{- if and $canary.trafficRouting (eq .role "api") }}
    canaryService: {{ .Release.Name }}-api-canary
    stableService: {{ .Release.Name }}-api
    trafficRouting:
      {{- toYaml $canary.trafficRouting | nindent 6 }}
    {{- end }}
    {{- end }}
{{- end -}}
