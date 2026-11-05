FROM scratch
ARG TARGETARCH
COPY --chmod=0755 ${TARGETARCH}/dtshim /dtshim
