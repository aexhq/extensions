export type PortableOptions = {
  delegation?: boolean;
};

export declare function portable(configuration?: PortableOptions): {
  name: "portable";
  version: "1.0.0";
  configuration: PortableOptions;
};
