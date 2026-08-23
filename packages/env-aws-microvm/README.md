# @aexhq/env-aws-microvm

Official retained Linux ARM64 computer environment backed by AWS Lambda MicroVMs.

The package contains both the public environment descriptor and the out-of-process provider,
guest supervisor, image builder, and egress gateway in `runtime/`. It implements the public Brain
environment protocol; applications use only `awsMicrovm()`.
