'use strict';

// Presentation shared by the issue list and the single-issue page.
const issue_ui = {
	SITE_LABELS : {
		WIKI : 'Wiki' ,
		WIKIDATA : 'Wikidata' ,
		GITHUB : 'GitHub' ,
		CODEBERG : 'Codeberg' ,
		BITBUCKET : 'Bitbucket' ,
		GITLAB : 'GitLab' ,
		GITLAB_WMF : 'GitLab (WMF)' ,
		TWITTER : 'Twitter' ,
	} ,

	SITE_ICONS : {
		WIKI : 'https://upload.wikimedia.org/wikipedia/commons/thumb/8/80/Wikipedia-logo-v2.svg/120px-Wikipedia-logo-v2.svg.png' ,
		WIKIDATA : 'https://upload.wikimedia.org/wikipedia/commons/thumb/f/ff/Wikidata-logo.svg/120px-Wikidata-logo.svg.png' ,
		TWITTER : 'https://upload.wikimedia.org/wikipedia/commons/thumb/9/9a/Font_Awesome_5_brands_Twitter.svg/120px-Font_Awesome_5_brands_Twitter.svg.png' ,
		GITHUB : 'https://upload.wikimedia.org/wikipedia/commons/thumb/c/c6/Font_Awesome_5_brands_github-square.svg/120px-Font_Awesome_5_brands_github-square.svg.png' ,
		CODEBERG : 'https://upload.wikimedia.org/wikipedia/commons/thumb/9/9a/Codeberg_logo.svg/120px-Codeberg_logo.svg.png' ,
		BITBUCKET : 'https://upload.wikimedia.org/wikipedia/commons/thumb/0/0e/Bitbucket-blue-logomark-only.svg/120px-Bitbucket-blue-logomark-only.svg.png' ,
	} ,

	STATUSES : {
		OPEN : { icon:'●' , severity:'warning' , label:'Open' } ,
		CLOSED : { icon:'✓' , severity:'good' , label:'Closed' } ,
	} ,

	PRIORITIES : {
		HIGH : { icon:'▲' , severity:'serious' , label:'High' } ,
		NORMAL : { icon:'■' , severity:'neutral' , label:'Normal' } ,
		LOW : { icon:'▽' , severity:'neutral' , label:'Low' } ,
	} ,

	siteIcon : function ( site ) { return this.SITE_ICONS[site] || '' } ,
	siteLabel : function ( site ) { return this.SITE_LABELS[site] || site } ,
	status : function ( status ) { return this.STATUSES[status] || { icon:'?' , severity:'neutral' , label:status } } ,
	priority : function ( priority ) { return this.PRIORITIES[priority] || { icon:'·' , severity:'neutral' , label:priority } } ,

	capitalize : function ( s ) {
		if ( typeof s !== 'string' ) return '' ;
		return s.charAt(0).toUpperCase() + s.slice(1).toLowerCase() ;
	} ,

	can_edit : function () {
		return typeof widar != 'undefined' && widar.is_logged_in && config.write_access.indexOf ( widar.getUserName() ) != -1 ;
	} ,

	// MySQL datetimes are UTC and come as "YYYY-MM-DD HH:MM:SS".
	parse : function ( timestamp ) {
		if ( !timestamp ) return null ;
		let date = new Date ( timestamp.replace ( ' ' , 'T' ) + 'Z' ) ;
		return isNaN ( date.getTime() ) ? null : date ;
	} ,

	ago : function ( timestamp ) {
		let date = this.parse ( timestamp ) ;
		if ( !date ) return timestamp || '–' ;
		return dash.duration ( ( Date.now() - date.getTime() ) / 1000 ) + ' ago' ;
	} ,

	// To the minute; the full timestamp goes in `title`.
	date : function ( timestamp ) { return timestamp ? timestamp.substr ( 0 , 16 ) : '–' } ,
} ;
