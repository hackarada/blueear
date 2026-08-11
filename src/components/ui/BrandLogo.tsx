import logoSymbol from "../../../assets/brand/logo-symbol.svg";

interface BrandLogoProps {
  size?: number;
}

export function BrandLogo({ size = 36 }: BrandLogoProps) {
  return (
    <img
      src={logoSymbol}
      width={size}
      height={size}
      alt=""
      aria-hidden="true"
      className="brand-logo"
      draggable={false}
    />
  );
}
